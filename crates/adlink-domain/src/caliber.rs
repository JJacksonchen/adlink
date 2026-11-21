//! Spend calibers (`消耗口径`), Performance calibers (`业绩口径`), and Custom calibers (`自定义口径`)
//! with deterministic financial reconciliation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::formula::{FormulaError, FormulaExpr};
use crate::money::Fen;

/// Standard Spend Calibers (`消耗口径`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendCaliber {
    /// 媒体账面消耗 = 现金消耗 + 赠款消耗
    #[default]
    GrossMediaCost,
    /// 现金消耗 = 仅统计实际现金扣款（不含赠款/返货券）
    CashCost,
    /// 净媒体消耗 = 账面消耗 × (1 - 返点率)
    NetAfterRebateCost,
    /// 全域投入成本 = 媒体账面消耗 + 达人商单费 + 平台服务费 + 内部创作成本
    FullFunnelCost,
}

/// Standard Performance / Revenue Calibers (`业绩口径`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceCaliber {
    /// 代理商毛流水 = 媒体账面消耗 + 服务费
    AgencyGrossBilling,
    /// 代理商净营收 = 媒体账面消耗 × 返点率 + 服务费
    AgencyNetRevenue,
    /// 品牌平台成交额 = 平台回传 GMV
    #[default]
    PlatformGmv,
    /// 品牌净 ROI = GMV / 净投入成本
    BrandNetRoi,
}

/// Category of a user-defined custom metric caliber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaliberCategory {
    Spend,
    Performance,
    Efficiency,
    Custom,
}

/// Persisted custom caliber formula definition (`自定义口径`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomCaliberDef {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub category: CaliberCategory,
    /// Safe formula expression, e.g. `"safe_div(gmv_yuan, cash_cost_yuan + kol_order_fee_yuan)"`.
    pub formula: String,
    pub unit: String,
    pub description: String,
    pub enabled: bool,
}

/// Aggregated raw metric inputs passed into the caliber evaluator.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaliberInput {
    pub gross_cost_fen: Fen,
    pub cash_cost_fen: Fen,
    pub grant_cost_fen: Fen,
    pub kol_order_fee_fen: Fen,
    pub platform_service_fee_fen: Fen,
    pub production_cost_fen: Fen,
    pub gmv_fen: Fen,
    pub impressions: u64,
    pub clicks: u64,
    pub interactions: u64,
    pub conversions: u64,
    pub leads: u64,
    pub orders: u64,
    pub store_verifies: u64,
    pub weighted_rebate_fen: Fen,
    pub weighted_agency_fee_fen: Fen,
}

impl CaliberInput {
    /// Exposes all standard variables in Yuan/count units for custom formula AST evaluation.
    pub fn to_var_map(&self) -> BTreeMap<String, f64> {
        let mut m = BTreeMap::new();
        let gross = self.gross_cost_fen.to_yuan_f64();
        let cash = self.cash_cost_fen.to_yuan_f64();
        let grant = self.grant_cost_fen.to_yuan_f64();
        let kol = self.kol_order_fee_fen.to_yuan_f64();
        let svc = self.platform_service_fee_fen.to_yuan_f64();
        let prod = self.production_cost_fen.to_yuan_f64();
        let gmv = self.gmv_fen.to_yuan_f64();
        let rebate = self.weighted_rebate_fen.to_yuan_f64();
        let agency_fee = self.weighted_agency_fee_fen.to_yuan_f64();

        m.insert("gross_cost_yuan".into(), gross);
        m.insert("cash_cost_yuan".into(), cash);
        m.insert("grant_cost_yuan".into(), grant);
        m.insert("kol_order_fee_yuan".into(), kol);
        m.insert("platform_service_fee_yuan".into(), svc);
        m.insert("production_cost_yuan".into(), prod);
        m.insert("gmv_yuan".into(), gmv);
        m.insert("rebate_yuan".into(), rebate);
        m.insert("agency_fee_yuan".into(), agency_fee);
        m.insert("net_after_rebate_yuan".into(), gross - rebate);
        m.insert("full_funnel_cost_yuan".into(), gross + kol + svc + prod);
        m.insert("agency_gross_billing_yuan".into(), gross + agency_fee);
        m.insert("agency_net_revenue_yuan".into(), rebate + agency_fee);
        m.insert("impressions".into(), self.impressions as f64);
        m.insert("clicks".into(), self.clicks as f64);
        m.insert("interactions".into(), self.interactions as f64);
        m.insert("conversions".into(), self.conversions as f64);
        m.insert("leads".into(), self.leads as f64);
        m.insert("orders".into(), self.orders as f64);
        m.insert("store_verifies".into(), self.store_verifies as f64);
        m
    }
}

/// Complete reconciliation snapshot comparing all Spend, Performance, and Custom calibers side by side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaliberReconciliation {
    pub label: String,
    pub gross_media_cost_yuan: f64,
    pub cash_cost_yuan: f64,
    pub grant_cost_yuan: f64,
    pub net_after_rebate_cost_yuan: f64,
    pub kol_and_service_cost_yuan: f64,
    pub production_cost_yuan: f64,
    pub full_funnel_cost_yuan: f64,
    pub agency_gross_billing_yuan: f64,
    pub agency_net_revenue_yuan: f64,
    pub platform_gmv_yuan: f64,
    pub gross_roi: f64,
    pub cash_roi: f64,
    pub full_funnel_roi: f64,
    pub ctr: f64,
    pub cpc_yuan: f64,
    pub cpm_yuan: f64,
    pub cvr: f64,
    pub cpa_yuan: f64,
    pub cpe_yuan: f64,
    pub impressions: u64,
    pub clicks: u64,
    pub interactions: u64,
    pub conversions: u64,
    pub orders: u64,
    #[serde(default)]
    pub custom_metrics: BTreeMap<String, f64>,
}

/// Evaluates standard & custom calibers cleanly without division-by-zero panics.
pub struct CaliberEvaluator;

impl CaliberEvaluator {
    /// Computes the full reconciliation breakdown and evaluates every enabled custom caliber.
    pub fn evaluate(
        label: impl Into<String>,
        input: &CaliberInput,
        custom_calibers: &[CustomCaliberDef],
    ) -> Result<CaliberReconciliation, FormulaError> {
        let vars = input.to_var_map();
        let gross = input.gross_cost_fen.to_yuan_f64();
        let cash = input.cash_cost_fen.to_yuan_f64();
        let grant = input.grant_cost_fen.to_yuan_f64();
        let rebate = input.weighted_rebate_fen.to_yuan_f64();
        let net_after_rebate = gross - rebate;
        let kol_and_svc = (input.kol_order_fee_fen + input.platform_service_fee_fen).to_yuan_f64();
        let prod = input.production_cost_fen.to_yuan_f64();
        let full_funnel = gross + kol_and_svc + prod;
        let agency_fee = input.weighted_agency_fee_fen.to_yuan_f64();
        let agency_gross_billing = gross + agency_fee;
        let agency_net_revenue = rebate + agency_fee;
        let gmv = input.gmv_fen.to_yuan_f64();

        let safe_div = |a: f64, b: f64| -> f64 {
            if b.abs() < 1e-12 {
                0.0
            } else {
                a / b
            }
        };

        let mut custom_metrics = BTreeMap::new();
        for def in custom_calibers.iter().filter(|d| d.enabled) {
            let expr = FormulaExpr::parse(&def.formula)?;
            let val = expr.eval(&vars)?;
            custom_metrics.insert(def.name.clone(), (val * 10000.0).round() / 10000.0);
        }

        let round2 = |v: f64| (v * 100.0).round() / 100.0;
        let round4 = |v: f64| (v * 10000.0).round() / 10000.0;

        Ok(CaliberReconciliation {
            label: label.into(),
            gross_media_cost_yuan: round2(gross),
            cash_cost_yuan: round2(cash),
            grant_cost_yuan: round2(grant),
            net_after_rebate_cost_yuan: round2(net_after_rebate),
            kol_and_service_cost_yuan: round2(kol_and_svc),
            production_cost_yuan: round2(prod),
            full_funnel_cost_yuan: round2(full_funnel),
            agency_gross_billing_yuan: round2(agency_gross_billing),
            agency_net_revenue_yuan: round2(agency_net_revenue),
            platform_gmv_yuan: round2(gmv),
            gross_roi: round4(safe_div(gmv, gross)),
            cash_roi: round4(safe_div(gmv, cash)),
            full_funnel_roi: round4(safe_div(gmv, full_funnel)),
            ctr: round4(safe_div(input.clicks as f64, input.impressions as f64)),
            cpc_yuan: round2(safe_div(gross, input.clicks as f64)),
            cpm_yuan: round2(safe_div(gross * 1000.0, input.impressions as f64)),
            cvr: round4(safe_div(input.conversions as f64, input.clicks as f64)),
            cpa_yuan: round2(safe_div(gross, input.conversions as f64)),
            cpe_yuan: round2(safe_div(gross + kol_and_svc, input.interactions as f64)),
            impressions: input.impressions,
            clicks: input.clicks,
            interactions: input.interactions,
            conversions: input.conversions,
            orders: input.orders,
            custom_metrics,
        })
    }

    /// Validates a custom caliber formula against a sample variable map before persisting it.
    pub fn validate_formula(formula: &str) -> Result<Vec<String>, FormulaError> {
        let expr = FormulaExpr::parse(formula)?;
        let sample = CaliberInput::default().to_var_map();
        // Ensure all referenced identifiers are known domain variables.
        let vars = expr.referenced_vars();
        for name in &vars {
            if !sample.contains_key(name) {
                return Err(FormulaError::UnknownIdentifier(name.clone()));
            }
        }
        let _ = expr.eval(&sample)?;
        Ok(vars)
    }
}
