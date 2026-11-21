//! Safe recursive-descent formula AST parser & evaluator for custom ad metric calibers.
//!
//! Zero shell/eval, deterministic recursion cap, division-by-zero safe via `safe_div(a, b)`,
//! and rich validation errors for user-authored custom calibers.

use std::collections::BTreeMap;

use thiserror::Error;

const MAX_DEPTH: usize = 32;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormulaError {
    #[error("empty formula")]
    Empty,
    #[error("unexpected token '{token}' at position {pos}")]
    UnexpectedToken { token: String, pos: usize },
    #[error("unexpected end of formula")]
    UnexpectedEof,
    #[error("unknown identifier '{0}'")]
    UnknownIdentifier(String),
    #[error("unknown function '{0}'")]
    UnknownFunction(String),
    #[error("function '{name}' expected {expected} arguments, got {actual}")]
    ArityMismatch {
        name: String,
        expected: usize,
        actual: usize,
    },
    #[error("division by zero")]
    DivisionByZero,
    #[error("formula nesting exceeds max depth {MAX_DEPTH}")]
    MaxDepthExceeded,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
    Gt,
    Gte,
    Lt,
    Lte,
    EqEq,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormulaExpr {
    Number(f64),
    Ident(String),
    UnaryNeg(Box<FormulaExpr>),
    Binary {
        op: BinOp,
        left: Box<FormulaExpr>,
        right: Box<FormulaExpr>,
    },
    Call {
        name: String,
        args: Vec<FormulaExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
}

impl FormulaExpr {
    /// Parses a formula string such as `"safe_div(gmv_yuan, cash_cost_yuan + kol_order_fee_yuan)"`.
    pub fn parse(input: &str) -> Result<Self, FormulaError> {
        let tokens = tokenize(input)?;
        if tokens.is_empty() {
            return Err(FormulaError::Empty);
        }
        let mut parser = Parser {
            tokens: &tokens,
            pos: 0,
        };
        let expr = parser.parse_cmp(0)?;
        if let Some((tok, pos)) = parser.peek() {
            return Err(FormulaError::UnexpectedToken {
                token: format!("{tok:?}"),
                pos,
            });
        }
        Ok(expr)
    }

    /// Evaluates the parsed AST against `vars`, returning `f64`.
    pub fn eval(&self, vars: &BTreeMap<String, f64>) -> Result<f64, FormulaError> {
        match self {
            Self::Number(v) => Ok(*v),
            Self::Ident(name) => vars
                .get(name)
                .copied()
                .ok_or_else(|| FormulaError::UnknownIdentifier(name.clone())),
            Self::UnaryNeg(inner) => Ok(-inner.eval(vars)?),
            Self::Binary { op, left, right } => {
                let l = left.eval(vars)?;
                let r = right.eval(vars)?;
                match op {
                    BinOp::Add => Ok(l + r),
                    BinOp::Sub => Ok(l - r),
                    BinOp::Mul => Ok(l * r),
                    BinOp::Div => {
                        if r.abs() < 1e-12 {
                            Err(FormulaError::DivisionByZero)
                        } else {
                            Ok(l / r)
                        }
                    }
                    BinOp::Gt => Ok(if l > r { 1.0 } else { 0.0 }),
                    BinOp::Gte => Ok(if l >= r { 1.0 } else { 0.0 }),
                    BinOp::Lt => Ok(if l < r { 1.0 } else { 0.0 }),
                    BinOp::Lte => Ok(if l <= r { 1.0 } else { 0.0 }),
                    BinOp::Eq => Ok(if (l - r).abs() < 1e-9 { 1.0 } else { 0.0 }),
                }
            }
            Self::Call { name, args } => eval_call(name, args, vars),
        }
    }

    /// Collects all variable names referenced inside this formula.
    pub fn referenced_vars(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_vars(&mut out);
        out.sort();
        out.dedup();
        out
    }

    fn collect_vars(&self, out: &mut Vec<String>) {
        match self {
            Self::Number(_) => {}
            Self::Ident(name) => out.push(name.clone()),
            Self::UnaryNeg(inner) => inner.collect_vars(out),
            Self::Binary { left, right, .. } => {
                left.collect_vars(out);
                right.collect_vars(out);
            }
            Self::Call { args, .. } => {
                for a in args {
                    a.collect_vars(out);
                }
            }
        }
    }
}

fn eval_call(
    name: &str,
    args: &[FormulaExpr],
    vars: &BTreeMap<String, f64>,
) -> Result<f64, FormulaError> {
    let check_arity = |expected: usize| -> Result<(), FormulaError> {
        if args.len() != expected {
            return Err(FormulaError::ArityMismatch {
                name: name.to_string(),
                expected,
                actual: args.len(),
            });
        }
        Ok(())
    };

    match name {
        "safe_div" => {
            check_arity(2)?;
            let num = args[0].eval(vars)?;
            let den = args[1].eval(vars)?;
            if den.abs() < 1e-12 {
                Ok(0.0)
            } else {
                Ok(num / den)
            }
        }
        "min" => {
            check_arity(2)?;
            Ok(args[0].eval(vars)?.min(args[1].eval(vars)?))
        }
        "max" => {
            check_arity(2)?;
            Ok(args[0].eval(vars)?.max(args[1].eval(vars)?))
        }
        "abs" => {
            check_arity(1)?;
            Ok(args[0].eval(vars)?.abs())
        }
        "round" => {
            check_arity(2)?;
            let val = args[0].eval(vars)?;
            let digits = args[1].eval(vars)?.clamp(0.0, 8.0) as i32;
            let factor = 10_f64.powi(digits);
            Ok((val * factor).round() / factor)
        }
        "if" => {
            check_arity(3)?;
            let cond = args[0].eval(vars)?;
            if cond.abs() > 1e-9 {
                args[1].eval(vars)
            } else {
                args[2].eval(vars)
            }
        }
        _ => Err(FormulaError::UnknownFunction(name.to_string())),
    }
}

fn tokenize(input: &str) -> Result<Vec<(Token, usize)>, FormulaError> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if ch.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        match ch {
            '+' => {
                out.push((Token::Plus, start));
                i += 1;
            }
            '-' => {
                out.push((Token::Minus, start));
                i += 1;
            }
            '*' => {
                out.push((Token::Star, start));
                i += 1;
            }
            '/' => {
                out.push((Token::Slash, start));
                i += 1;
            }
            '(' => {
                out.push((Token::LParen, start));
                i += 1;
            }
            ')' => {
                out.push((Token::RParen, start));
                i += 1;
            }
            ',' => {
                out.push((Token::Comma, start));
                i += 1;
            }
            '>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push((Token::Gte, start));
                    i += 2;
                } else {
                    out.push((Token::Gt, start));
                    i += 1;
                }
            }
            '<' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push((Token::Lte, start));
                    i += 2;
                } else {
                    out.push((Token::Lt, start));
                    i += 1;
                }
            }
            '=' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push((Token::EqEq, start));
                    i += 2;
                } else {
                    return Err(FormulaError::UnexpectedToken {
                        token: "=".into(),
                        pos: start,
                    });
                }
            }
            '0'..='9' | '.' => {
                let mut seen_dot = ch == '.';
                i += 1;
                while i < bytes.len() {
                    let c = bytes[i] as char;
                    if c.is_ascii_digit() {
                        i += 1;
                    } else if c == '.' && !seen_dot {
                        seen_dot = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
                let raw = &input[start..i];
                let num: f64 = raw.parse().map_err(|_| FormulaError::UnexpectedToken {
                    token: raw.to_string(),
                    pos: start,
                })?;
                out.push((Token::Number(num), start));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                i += 1;
                while i < bytes.len() {
                    let c2 = bytes[i] as char;
                    if c2.is_ascii_alphanumeric() || c2 == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push((Token::Ident(input[start..i].to_string()), start));
            }
            _ => {
                return Err(FormulaError::UnexpectedToken {
                    token: ch.to_string(),
                    pos: start,
                });
            }
        }
    }
    Ok(out)
}

struct Parser<'a> {
    tokens: &'a [(Token, usize)],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<(&Token, usize)> {
        self.tokens.get(self.pos).map(|(t, p)| (t, *p))
    }

    fn next(&mut self) -> Option<(&Token, usize)> {
        let item = self.tokens.get(self.pos).map(|(t, p)| (t, *p));
        if item.is_some() {
            self.pos += 1;
        }
        item
    }

    fn parse_cmp(&mut self, depth: usize) -> Result<FormulaExpr, FormulaError> {
        if depth > MAX_DEPTH {
            return Err(FormulaError::MaxDepthExceeded);
        }
        let mut expr = self.parse_add_sub(depth + 1)?;
        while let Some((tok, _)) = self.peek() {
            let op = match tok {
                Token::Gt => BinOp::Gt,
                Token::Gte => BinOp::Gte,
                Token::Lt => BinOp::Lt,
                Token::Lte => BinOp::Lte,
                Token::EqEq => BinOp::Eq,
                _ => break,
            };
            self.next();
            let right = self.parse_add_sub(depth + 1)?;
            expr = FormulaExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_add_sub(&mut self, depth: usize) -> Result<FormulaExpr, FormulaError> {
        if depth > MAX_DEPTH {
            return Err(FormulaError::MaxDepthExceeded);
        }
        let mut expr = self.parse_mul_div(depth + 1)?;
        while let Some((tok, _)) = self.peek() {
            let op = match tok {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.next();
            let right = self.parse_mul_div(depth + 1)?;
            expr = FormulaExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_mul_div(&mut self, depth: usize) -> Result<FormulaExpr, FormulaError> {
        if depth > MAX_DEPTH {
            return Err(FormulaError::MaxDepthExceeded);
        }
        let mut expr = self.parse_unary(depth + 1)?;
        while let Some((tok, _)) = self.peek() {
            let op = match tok {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                _ => break,
            };
            self.next();
            let right = self.parse_unary(depth + 1)?;
            expr = FormulaExpr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self, depth: usize) -> Result<FormulaExpr, FormulaError> {
        if depth > MAX_DEPTH {
            return Err(FormulaError::MaxDepthExceeded);
        }
        if let Some((Token::Minus, _)) = self.peek() {
            self.next();
            let inner = self.parse_unary(depth + 1)?;
            return Ok(FormulaExpr::UnaryNeg(Box::new(inner)));
        }
        self.parse_primary(depth + 1)
    }

    fn parse_primary(&mut self, depth: usize) -> Result<FormulaExpr, FormulaError> {
        if depth > MAX_DEPTH {
            return Err(FormulaError::MaxDepthExceeded);
        }
        let (tok, pos) = self.next().ok_or(FormulaError::UnexpectedEof)?;
        match tok.clone() {
            Token::Number(n) => Ok(FormulaExpr::Number(n)),
            Token::Ident(name) => {
                if let Some((Token::LParen, _)) = self.peek() {
                    self.next();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some((Token::RParen, _))) {
                        loop {
                            args.push(self.parse_cmp(depth + 1)?);
                            match self.peek() {
                                Some((Token::Comma, _)) => {
                                    self.next();
                                }
                                Some((Token::RParen, _)) => break,
                                Some((other, p)) => {
                                    return Err(FormulaError::UnexpectedToken {
                                        token: format!("{other:?}"),
                                        pos: p,
                                    });
                                }
                                None => return Err(FormulaError::UnexpectedEof),
                            }
                        }
                    }
                    match self.next() {
                        Some((Token::RParen, _)) => {}
                        _ => return Err(FormulaError::UnexpectedEof),
                    }
                    Ok(FormulaExpr::Call { name, args })
                } else {
                    Ok(FormulaExpr::Ident(name))
                }
            }
            Token::LParen => {
                let inner = self.parse_cmp(depth + 1)?;
                match self.next() {
                    Some((Token::RParen, _)) => Ok(inner),
                    Some((other, p)) => Err(FormulaError::UnexpectedToken {
                        token: format!("{other:?}"),
                        pos: p,
                    }),
                    None => Err(FormulaError::UnexpectedEof),
                }
            }
            other => Err(FormulaError::UnexpectedToken {
                token: format!("{other:?}"),
                pos,
            }),
        }
    }
}
