//! Canonical domain models for AdLink (`adlink-domain`).

pub mod ad_ops;
pub mod caliber;
pub mod creative;
pub mod formula;
pub mod governance;
pub mod money;
pub mod platform;
pub mod report;

pub use ad_ops::{AdEntity, AdMutationAction, AdMutationRequest, AdMutationResult, AuditLogEntry};
pub use caliber::{
    CaliberEvaluator, CaliberInput, CaliberReconciliation, CaliberCategory, CustomCaliberDef,
    PerformanceCaliber, SpendCaliber,
};
pub use creative::{
    CreativeAsset, CreativeAttributionRow, CreativeFormat, CreativeHookType, CreativePlatformLink,
    CreativeProductionContribution, CreativeTaxonomy, CreatorWorkloadSummary, ProductionRole,
    SeedingBoostAttributionRow,
};
pub use formula::{FormulaError, FormulaExpr};
pub use governance::{
    AccountType, AdAccount, BindingRole, OrgType, Organization, Person, PersonAccountBinding,
    PersonRole, Project, ProjectAccountLink, ProjectType, SettlementMode,
};
pub use money::Fen;
pub use platform::{ApiAccessMode, OfficialFieldMapping, ParsePlatformError, PlatformId, PlatformMeta};
pub use report::{
    DailyFundStatement, QueryDimension, ReportGranularity, ReportLevel, ReportQuery,
    SyncCheckpoint, SyncJobRecord, TimeLine, UnifiedReportRow,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fen_conversions_preserve_exact_cents() {
        let f = Fen::from_yuan_f64(123.456);
        assert_eq!(f.as_fen(), 12346);
        assert_eq!(format!("{}", Fen::from_fen(-4205)), "-42.05");
        assert_eq!(Fen::from_fen(1000).scale(0.6).as_fen(), 600);
    }

    #[test]
    fn formula_ast_evaluates_safely() {
        let vars = CaliberEvaluator::validate_formula(
            "round(safe_div(gmv_yuan, cash_cost_yuan + kol_order_fee_yuan), 2)",
        )
        .unwrap();
        assert_eq!(
            vars,
            vec!["cash_cost_yuan", "gmv_yuan", "kol_order_fee_yuan"]
        );

        let expr =
            FormulaExpr::parse("if(gmv_yuan > 1000, safe_div(gmv_yuan, cash_cost_yuan), 0)")
                .unwrap();
        let mut map = CaliberInput {
            cash_cost_fen: Fen::from_yuan_f64(500.0),
            gmv_fen: Fen::from_yuan_f64(2500.0),
            ..Default::default()
        }
        .to_var_map();
        assert!((expr.eval(&map).unwrap() - 5.0).abs() < 1e-9);

        map.insert("cash_cost_yuan".into(), 0.0);
        // safe_div must never panic or return NaN/Inf on 0 denominator.
        assert_eq!(expr.eval(&map).unwrap(), 0.0);
    }

    #[test]
    fn caliber_reconciliation_computes_spend_and_performance_views() {
        let input = CaliberInput {
            gross_cost_fen: Fen::from_yuan_f64(10_000.0),
            cash_cost_fen: Fen::from_yuan_f64(8_500.0),
            grant_cost_fen: Fen::from_yuan_f64(1_500.0),
            kol_order_fee_fen: Fen::from_yuan_f64(2_000.0),
            platform_service_fee_fen: Fen::from_yuan_f64(200.0),
            production_cost_fen: Fen::from_yuan_f64(800.0),
            gmv_fen: Fen::from_yuan_f64(39_000.0),
            impressions: 500_000,
            clicks: 15_000,
            interactions: 4_000,
            conversions: 600,
            leads: 0,
            orders: 450,
            store_verifies: 0,
            weighted_rebate_fen: Fen::from_yuan_f64(600.0),
            weighted_agency_fee_fen: Fen::from_yuan_f64(500.0),
        };
        let custom = vec![CustomCaliberDef {
            id: "c1".into(),
            name: "effective_net_roi".into(),
            display_name: "净投入ROI".into(),
            category: CaliberCategory::Custom,
            formula: "safe_div(gmv_yuan, net_after_rebate_yuan + kol_order_fee_yuan)".into(),
            unit: "ratio".into(),
            description: "GMV / (净媒体消耗 + 达人商单)".into(),
            enabled: true,
        }];
        let rec = CaliberEvaluator::evaluate("Q4", &input, &custom).unwrap();
        assert_eq!(rec.gross_media_cost_yuan, 10_000.0);
        assert_eq!(rec.cash_cost_yuan, 8_500.0);
        assert_eq!(rec.net_after_rebate_cost_yuan, 9_400.0);
        assert_eq!(rec.full_funnel_cost_yuan, 13_000.0);
        assert_eq!(rec.agency_net_revenue_yuan, 1_100.0);
        assert_eq!(rec.gross_roi, 3.9);
        assert_eq!(rec.full_funnel_roi, 3.0);
        assert!(rec.custom_metrics.contains_key("effective_net_roi"));
    }

    #[test]
    fn all_ten_platforms_have_metadata_and_mappings() {
        assert_eq!(PlatformId::ALL.len(), 10);
        for p in PlatformId::ALL {
            let meta = p.meta();
            assert!(!meta.field_mappings.is_empty(), "{p} missing field mappings");
            assert!(meta.max_sync_days_per_chunk >= 1);
            assert!(meta.default_qps > 0.0);
        }
    }
}
