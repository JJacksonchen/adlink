//! SQLite WAL persistence backend implementing [`crate::StorageService`] and [`adlink_kernel::Plugin`].

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use adlink_domain::{
    AccountType, AdAccount, AdEntity, AuditLogEntry, BindingRole, CaliberCategory,
    CaliberReconciliation, CreativeAsset, CreativeAttributionRow, CreativeFormat, CreativeHookType,
    CreativePlatformLink, CreativeProductionContribution, CreativeTaxonomy, CreatorWorkloadSummary,
    CustomCaliberDef, DailyFundStatement, Fen, OrgType, Organization, Person,
    PersonAccountBinding, PersonRole, PlatformId, ProductionRole, Project, ProjectAccountLink,
    ProjectType, ReportGranularity, ReportLevel, ReportQuery, SeedingBoostAttributionRow,
    SettlementMode, SyncCheckpoint, SyncJobRecord, TimeLine, UnifiedReportRow,
};
use adlink_kernel::{KernelError, KernelResult, Plugin, PluginCategory, PluginContext, PluginHealth, PluginManifest};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::migrations::{configure_connection, run_migrations};
use crate::queries::{compute_creative_attribution, evaluate_report_query, parse_date, AttributionUniverse};
use crate::{StorageError, StorageResult, StorageService};

#[derive(Debug, Clone, Deserialize)]
pub struct SqlitePluginConfig {
    pub db_path: String,
}

/// Production SQLite WAL storage engine packaged as a kernel plugin.
pub struct SqliteStorage {
    db_path: PathBuf,
    writer: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    /// Opens or initializes the SQLite database at `path` (supports `:memory:` or file paths).
    pub fn open(path: impl Into<PathBuf>) -> StorageResult<Self> {
        let db_path = path.into();
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() && db_path.to_string_lossy() != ":memory:" {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = if db_path.to_string_lossy() == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(&db_path)?
        };
        run_migrations(&conn)?;
        Ok(Self {
            db_path,
            writer: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs a blocking SQLite operation on the dedicated Tokio blocking pool.
    async fn with_conn<F, R>(&self, f: F) -> StorageResult<R>
    where
        F: FnOnce(&mut Connection) -> StorageResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || {
            let mut guard = writer
                .lock()
                .map_err(|_| StorageError::DataIntegrity("sqlite mutex poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| StorageError::DataIntegrity(format!("blocking join failed: {e}")))?
    }

    pub fn db_path(&self) -> &PathBuf {
        &self.db_path
    }
}

/// Kernel plugin wrapper registering `SqliteStorage` as the active `Arc<dyn StorageService>`.
pub struct SqliteStoragePlugin {
    storage: Arc<SqliteStorage>,
}

impl SqliteStoragePlugin {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl Plugin for SqliteStoragePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "storage.sqlite_wal".into(),
            name: "SQLite WAL Storage Engine".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Transactional SQLite WAL persistence for governance, reports, checkpoints, and audit logs".into(),
            category: PluginCategory::Storage,
            provides: vec!["storage".into()],
            requires: vec![],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": { "db_path": { "type": "string" } }
            }),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn StorageService> = Arc::clone(&self.storage) as Arc<dyn StorageService>;
        ctx.provide_service(svc);
        Ok(())
    }

    async fn health(&self) -> PluginHealth {
        match self.storage.count_report_rows().await {
            Ok(rows) => PluginHealth {
                healthy: true,
                status: "ok".into(),
                detail: Some(format!("sqlite wal online ({rows} report rows)")),
                checked_at: Utc::now(),
            },
            Err(err) => PluginHealth::unhealthy(err.to_string()),
        }
    }
}

/// Helper to create & mount a `SqliteStoragePlugin` directly from config.
pub fn open_sqlite_from_config(cfg: &Value) -> KernelResult<Arc<SqliteStorage>> {
    let path = cfg
        .get("db_path")
        .and_then(Value::as_str)
        .unwrap_or("data/adlink.db");
    SqliteStorage::open(path).map(Arc::new).map_err(|e| KernelError::MountFailed {
        plugin: "storage.sqlite_wal".into(),
        message: e.to_string(),
    })
}

fn to_json<T: Serialize>(v: &T) -> StorageResult<String> {
    serde_json::to_string(v).map_err(|e| StorageError::DataIntegrity(e.to_string()))
}

fn parse_enum_str<T: serde::de::DeserializeOwned>(raw: &str) -> StorageResult<T> {
    serde_json::from_str(&format!("\"{raw}\""))
        .or_else(|_| serde_json::from_str(raw))
        .map_err(|e| StorageError::DataIntegrity(format!("enum decode error for '{raw}': {e}")))
}

fn serialize_enum_str<T: Serialize>(v: &T) -> StorageResult<String> {
    let val = serde_json::to_value(v).map_err(|e| StorageError::DataIntegrity(e.to_string()))?;
    match val {
        Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

#[async_trait]
impl StorageService for SqliteStorage {
    async fn upsert_organization(&self, org: &Organization) -> StorageResult<()> {
        let org = org.clone();
        self.with_conn(move |conn| {
            let org_type = serialize_enum_str(&org.org_type)?;
            conn.execute(
                "INSERT INTO organizations (id, name, org_type, default_currency)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    org_type = excluded.org_type,
                    default_currency = excluded.default_currency",
                params![org.id, org.name, org_type, org.default_currency],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_organizations(&self) -> StorageResult<Vec<Organization>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, org_type, default_currency FROM organizations ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, name, ot, curr) = row?;
                out.push(Organization {
                    id,
                    name,
                    org_type: parse_enum_str::<OrgType>(&ot)?,
                    default_currency: curr,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn upsert_person(&self, person: &Person) -> StorageResult<()> {
        let p = person.clone();
        self.with_conn(move |conn| {
            let role = serialize_enum_str(&p.role)?;
            conn.execute(
                "INSERT INTO people (id, org_id, name, email, department, role, monthly_cost_fen, active)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                    org_id = excluded.org_id,
                    name = excluded.name,
                    email = excluded.email,
                    department = excluded.department,
                    role = excluded.role,
                    monthly_cost_fen = excluded.monthly_cost_fen,
                    active = excluded.active",
                params![
                    p.id,
                    p.org_id,
                    p.name,
                    p.email,
                    p.department,
                    role,
                    p.monthly_cost_fen.as_fen(),
                    if p.active { 1 } else { 0 }
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_people(&self) -> StorageResult<Vec<Person>> {
        self.with_conn(|conn| load_people(conn)).await
    }

    async fn upsert_ad_account(&self, acct: &AdAccount) -> StorageResult<()> {
        let a = acct.clone();
        self.with_conn(move |conn| {
            let acct_type = serialize_enum_str(&a.account_type)?;
            let extra = to_json(&a.extra)?;
            conn.execute(
                "INSERT INTO ad_accounts (
                    id, org_id, platform, platform_account_id, account_name, account_type,
                    rebate_rate, service_fee_rate, default_cash_ratio, status, credential_ref, extra_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                    org_id = excluded.org_id,
                    platform = excluded.platform,
                    platform_account_id = excluded.platform_account_id,
                    account_name = excluded.account_name,
                    account_type = excluded.account_type,
                    rebate_rate = excluded.rebate_rate,
                    service_fee_rate = excluded.service_fee_rate,
                    default_cash_ratio = excluded.default_cash_ratio,
                    status = excluded.status,
                    credential_ref = excluded.credential_ref,
                    extra_json = excluded.extra_json",
                params![
                    a.id,
                    a.org_id,
                    a.platform.as_str(),
                    a.platform_account_id,
                    a.account_name,
                    acct_type,
                    a.rebate_rate,
                    a.service_fee_rate,
                    a.default_cash_ratio,
                    a.status,
                    a.credential_ref,
                    extra
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_ad_accounts(&self, platform: Option<PlatformId>) -> StorageResult<Vec<AdAccount>> {
        self.with_conn(move |conn| {
            let mut all = load_accounts(conn)?;
            if let Some(p) = platform {
                all.retain(|a| a.platform == p);
            }
            Ok(all)
        })
        .await
    }

    async fn upsert_person_account_binding(
        &self,
        binding: &PersonAccountBinding,
    ) -> StorageResult<()> {
        let b = binding.clone();
        self.with_conn(move |conn| {
            let role = serialize_enum_str(&b.relation_role)?;
            conn.execute(
                "INSERT INTO person_account_bindings (
                    id, person_id, ad_account_id, relation_role, split_weight, effective_from, effective_to
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    person_id = excluded.person_id,
                    ad_account_id = excluded.ad_account_id,
                    relation_role = excluded.relation_role,
                    split_weight = excluded.split_weight,
                    effective_from = excluded.effective_from,
                    effective_to = excluded.effective_to",
                params![
                    b.id,
                    b.person_id,
                    b.ad_account_id,
                    role,
                    b.split_weight,
                    b.effective_from.format("%Y-%m-%d").to_string(),
                    b.effective_to.map(|d| d.format("%Y-%m-%d").to_string())
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_person_account_bindings(&self) -> StorageResult<Vec<PersonAccountBinding>> {
        self.with_conn(|conn| load_bindings(conn)).await
    }

    async fn upsert_project(&self, project: &Project) -> StorageResult<()> {
        let p = project.clone();
        self.with_conn(move |conn| {
            let pt = serialize_enum_str(&p.project_type)?;
            let sm = serialize_enum_str(&p.settlement_mode)?;
            let tags = to_json(&p.tags)?;
            conn.execute(
                "INSERT INTO projects (
                    id, org_id, code, name, client_name, brand_name, project_type,
                    business_line, settlement_mode, budget_fen, target_roi, target_cpa_fen,
                    owner_person_id, start_date, end_date, tags_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(id) DO UPDATE SET
                    org_id = excluded.org_id,
                    code = excluded.code,
                    name = excluded.name,
                    client_name = excluded.client_name,
                    brand_name = excluded.brand_name,
                    project_type = excluded.project_type,
                    business_line = excluded.business_line,
                    settlement_mode = excluded.settlement_mode,
                    budget_fen = excluded.budget_fen,
                    target_roi = excluded.target_roi,
                    target_cpa_fen = excluded.target_cpa_fen,
                    owner_person_id = excluded.owner_person_id,
                    start_date = excluded.start_date,
                    end_date = excluded.end_date,
                    tags_json = excluded.tags_json",
                params![
                    p.id,
                    p.org_id,
                    p.code,
                    p.name,
                    p.client_name,
                    p.brand_name,
                    pt,
                    p.business_line,
                    sm,
                    p.budget_fen.as_fen(),
                    p.target_roi,
                    p.target_cpa_fen.map(Fen::as_fen),
                    p.owner_person_id,
                    p.start_date.format("%Y-%m-%d").to_string(),
                    p.end_date.map(|d| d.format("%Y-%m-%d").to_string()),
                    tags
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_projects(&self) -> StorageResult<Vec<Project>> {
        self.with_conn(|conn| load_projects(conn)).await
    }

    async fn upsert_project_account_link(&self, link: &ProjectAccountLink) -> StorageResult<()> {
        let l = link.clone();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO project_account_links (id, project_id, ad_account_id, campaign_filter_regex, allocation_weight)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    project_id = excluded.project_id,
                    ad_account_id = excluded.ad_account_id,
                    campaign_filter_regex = excluded.campaign_filter_regex,
                    allocation_weight = excluded.allocation_weight",
                params![
                    l.id,
                    l.project_id,
                    l.ad_account_id,
                    l.campaign_filter_regex,
                    l.allocation_weight
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_project_account_links(&self) -> StorageResult<Vec<ProjectAccountLink>> {
        self.with_conn(|conn| load_project_links(conn)).await
    }

    async fn upsert_creative_asset(&self, asset: &CreativeAsset) -> StorageResult<()> {
        let a = asset.clone();
        self.with_conn(move |conn| {
            let fmt = serialize_enum_str(&a.taxonomy.format)?;
            let hook = serialize_enum_str(&a.taxonomy.hook_type)?;
            let custom_tags = to_json(&a.taxonomy.custom_tags)?;
            conn.execute(
                "INSERT INTO creative_assets (
                    id, project_id, asset_code, title, format, hook_type, scene_tag,
                    selling_point, tone, custom_tags_json, production_cost_fen, production_hours,
                    created_date, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(id) DO UPDATE SET
                    project_id = excluded.project_id,
                    asset_code = excluded.asset_code,
                    title = excluded.title,
                    format = excluded.format,
                    hook_type = excluded.hook_type,
                    scene_tag = excluded.scene_tag,
                    selling_point = excluded.selling_point,
                    tone = excluded.tone,
                    custom_tags_json = excluded.custom_tags_json,
                    production_cost_fen = excluded.production_cost_fen,
                    production_hours = excluded.production_hours,
                    created_date = excluded.created_date,
                    status = excluded.status",
                params![
                    a.id,
                    a.project_id,
                    a.asset_code,
                    a.title,
                    fmt,
                    hook,
                    a.taxonomy.scene_tag,
                    a.taxonomy.selling_point,
                    a.taxonomy.tone,
                    custom_tags,
                    a.production_cost_fen.as_fen(),
                    a.production_hours,
                    a.created_date.format("%Y-%m-%d").to_string(),
                    a.status
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_creative_assets(&self) -> StorageResult<Vec<CreativeAsset>> {
        self.with_conn(|conn| load_creative_assets(conn)).await
    }

    async fn upsert_creative_platform_link(&self, link: &CreativePlatformLink) -> StorageResult<()> {
        let l = link.clone();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO creative_platform_links (id, asset_id, platform, ad_account_id, platform_creative_id, platform_content_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(platform, ad_account_id, platform_creative_id) DO UPDATE SET
                    asset_id = excluded.asset_id,
                    platform_content_id = excluded.platform_content_id",
                params![
                    l.id,
                    l.asset_id,
                    l.platform.as_str(),
                    l.ad_account_id,
                    l.platform_creative_id,
                    l.platform_content_id
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_creative_platform_links(&self) -> StorageResult<Vec<CreativePlatformLink>> {
        self.with_conn(|conn| load_creative_links(conn)).await
    }

    async fn upsert_creative_contribution(
        &self,
        contrib: &CreativeProductionContribution,
    ) -> StorageResult<()> {
        let c = contrib.clone();
        self.with_conn(move |conn| {
            let role = serialize_enum_str(&c.role)?;
            conn.execute(
                "INSERT INTO creative_production_contributions (id, asset_id, person_id, role, work_hours, credit_weight, piece_cost_fen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    asset_id = excluded.asset_id,
                    person_id = excluded.person_id,
                    role = excluded.role,
                    work_hours = excluded.work_hours,
                    credit_weight = excluded.credit_weight,
                    piece_cost_fen = excluded.piece_cost_fen",
                params![
                    c.id,
                    c.asset_id,
                    c.person_id,
                    role,
                    c.work_hours,
                    c.credit_weight,
                    c.piece_cost_fen.as_fen()
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_creative_contributions(
        &self,
    ) -> StorageResult<Vec<CreativeProductionContribution>> {
        self.with_conn(|conn| load_creative_contributions(conn)).await
    }

    async fn upsert_custom_caliber(&self, caliber: &CustomCaliberDef) -> StorageResult<()> {
        let c = caliber.clone();
        self.with_conn(move |conn| {
            let cat = serialize_enum_str(&c.category)?;
            conn.execute(
                "INSERT INTO custom_calibers (id, name, display_name, category, formula, unit, description, enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(name) DO UPDATE SET
                    display_name = excluded.display_name,
                    category = excluded.category,
                    formula = excluded.formula,
                    unit = excluded.unit,
                    description = excluded.description,
                    enabled = excluded.enabled",
                params![
                    c.id,
                    c.name,
                    c.display_name,
                    cat,
                    c.formula,
                    c.unit,
                    c.description,
                    if c.enabled { 1 } else { 0 }
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_custom_calibers(&self) -> StorageResult<Vec<CustomCaliberDef>> {
        self.with_conn(|conn| load_custom_calibers(conn)).await
    }

    async fn upsert_reports(&self, rows: &[UnifiedReportRow]) -> StorageResult<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let owned = rows.to_vec();
        self.with_conn(move |conn| {
            let mut total = 0usize;
            // Chunk inside BEGIN IMMEDIATE transactions to sustain high throughput under WAL.
            for chunk in owned.chunks(500) {
                let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                {
                    let mut stmt = tx.prepare_cached(
                        "INSERT INTO unified_reports (
                            platform, ad_account_id, platform_account_id, stat_date, stat_hour,
                            granularity, level, entity_id, entity_name, parent_entity_id, content_id,
                            time_line, gross_cost_fen, cash_cost_fen, grant_cost_fen,
                            kol_order_fee_fen, platform_service_fee_fen, impressions, clicks,
                            conversions, deep_conversions, leads, orders, gmv_fen, interactions,
                            video_plays, effective_plays, new_follows, store_verifies, raw_fields_json
                         ) VALUES (
                            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                            ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30
                         )
                         ON CONFLICT(platform, ad_account_id, stat_date, stat_hour, granularity, level, entity_id, time_line)
                         DO UPDATE SET
                            entity_name = excluded.entity_name,
                            parent_entity_id = excluded.parent_entity_id,
                            content_id = COALESCE(excluded.content_id, unified_reports.content_id),
                            gross_cost_fen = excluded.gross_cost_fen,
                            cash_cost_fen = excluded.cash_cost_fen,
                            grant_cost_fen = excluded.grant_cost_fen,
                            kol_order_fee_fen = excluded.kol_order_fee_fen,
                            platform_service_fee_fen = excluded.platform_service_fee_fen,
                            impressions = excluded.impressions,
                            clicks = excluded.clicks,
                            conversions = excluded.conversions,
                            deep_conversions = excluded.deep_conversions,
                            leads = excluded.leads,
                            orders = excluded.orders,
                            gmv_fen = excluded.gmv_fen,
                            interactions = excluded.interactions,
                            video_plays = excluded.video_plays,
                            effective_plays = excluded.effective_plays,
                            new_follows = excluded.new_follows,
                            store_verifies = excluded.store_verifies,
                            raw_fields_json = excluded.raw_fields_json,
                            updated_at = datetime('now')",
                    )?;
                    for r in chunk {
                        let raw_json = to_json(&r.raw_fields)?;
                        stmt.execute(params![
                            r.platform.as_str(),
                            r.ad_account_id,
                            r.platform_account_id,
                            r.stat_date.format("%Y-%m-%d").to_string(),
                            r.stat_hour,
                            r.granularity.as_str(),
                            r.level.as_str(),
                            r.entity_id,
                            r.entity_name,
                            r.parent_entity_id,
                            r.content_id,
                            r.time_line.as_str(),
                            r.gross_cost_fen.as_fen(),
                            r.cash_cost_fen.as_fen(),
                            r.grant_cost_fen.as_fen(),
                            r.kol_order_fee_fen.as_fen(),
                            r.platform_service_fee_fen.as_fen(),
                            r.impressions as i64,
                            r.clicks as i64,
                            r.conversions as i64,
                            r.deep_conversions as i64,
                            r.leads as i64,
                            r.orders as i64,
                            r.gmv_fen.as_fen(),
                            r.interactions as i64,
                            r.video_plays as i64,
                            r.effective_plays as i64,
                            r.new_follows as i64,
                            r.store_verifies as i64,
                            raw_json
                        ])?;
                        total += 1;
                    }
                }
                tx.commit()?;
            }
            Ok(total)
        })
        .await
    }

    async fn upsert_fund_statements(&self, rows: &[DailyFundStatement]) -> StorageResult<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let owned = rows.to_vec();
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut total = 0usize;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO daily_fund_statements (
                        platform, ad_account_id, platform_account_id, stat_date,
                        cash_balance_fen, grant_balance_fen, cash_cost_fen, grant_cost_fen,
                        shared_wallet_cost_fen, income_fen, raw_fields_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(platform, ad_account_id, stat_date) DO UPDATE SET
                        cash_balance_fen = excluded.cash_balance_fen,
                        grant_balance_fen = excluded.grant_balance_fen,
                        cash_cost_fen = excluded.cash_cost_fen,
                        grant_cost_fen = excluded.grant_cost_fen,
                        shared_wallet_cost_fen = excluded.shared_wallet_cost_fen,
                        income_fen = excluded.income_fen,
                        raw_fields_json = excluded.raw_fields_json",
                )?;
                for r in &owned {
                    let raw_json = to_json(&r.raw_fields)?;
                    stmt.execute(params![
                        r.platform.as_str(),
                        r.ad_account_id,
                        r.platform_account_id,
                        r.stat_date.format("%Y-%m-%d").to_string(),
                        r.cash_balance_fen.as_fen(),
                        r.grant_balance_fen.as_fen(),
                        r.cash_cost_fen.as_fen(),
                        r.grant_cost_fen.as_fen(),
                        r.shared_wallet_cost_fen.as_fen(),
                        r.income_fen.as_fen(),
                        raw_json
                    ])?;
                    total += 1;
                }
            }
            tx.commit()?;
            Ok(total)
        })
        .await
    }

    async fn list_fund_statements(
        &self,
        start_date: NaiveDate,
        end_date: NaiveDate,
        ad_account_id: Option<&str>,
    ) -> StorageResult<Vec<DailyFundStatement>> {
        let acct = ad_account_id.map(str::to_string);
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT platform, ad_account_id, platform_account_id, stat_date,
                        cash_balance_fen, grant_balance_fen, cash_cost_fen, grant_cost_fen,
                        shared_wallet_cost_fen, income_fen, raw_fields_json
                 FROM daily_fund_statements
                 WHERE stat_date >= ?1 AND stat_date <= ?2
                   AND (?3 IS NULL OR ad_account_id = ?3)
                 ORDER BY stat_date ASC, ad_account_id ASC",
            )?;
            let rows = stmt.query_map(
                params![
                    start_date.format("%Y-%m-%d").to_string(),
                    end_date.format("%Y-%m-%d").to_string(),
                    acct
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, i64>(8)?,
                        r.get::<_, i64>(9)?,
                        r.get::<_, String>(10)?,
                    ))
                },
            )?;
            let mut out = Vec::new();
            for row in rows {
                let (p, acct_id, plat_acct, dt, cb, gb, cc, gc, swc, inc, raw) = row?;
                out.push(DailyFundStatement {
                    platform: p
                        .parse()
                        .map_err(|e: adlink_domain::ParsePlatformError| StorageError::DataIntegrity(e.to_string()))?,
                    ad_account_id: acct_id,
                    platform_account_id: plat_acct,
                    stat_date: parse_date(&dt)?,
                    cash_balance_fen: Fen(cb),
                    grant_balance_fen: Fen(gb),
                    cash_cost_fen: Fen(cc),
                    grant_cost_fen: Fen(gc),
                    shared_wallet_cost_fen: Fen(swc),
                    income_fen: Fen(inc),
                    raw_fields: serde_json::from_str(&raw).unwrap_or_default(),
                });
            }
            Ok(out)
        })
        .await
    }

    async fn query_reports(&self, query: &ReportQuery) -> StorageResult<Vec<UnifiedReportRow>> {
        let q = query.clone();
        self.with_conn(move |conn| load_reports_filtered(conn, &q))
            .await
    }

    async fn query_reconciled_metrics(
        &self,
        query: &ReportQuery,
    ) -> StorageResult<(CaliberReconciliation, Vec<CaliberReconciliation>)> {
        let q = query.clone();
        self.with_conn(move |conn| {
            let rows = load_reports_filtered(conn, &q)?;
            let universe = load_universe(conn, q.start_date, q.end_date)?;
            evaluate_report_query(&rows, &q, &universe)
        })
        .await
    }

    async fn query_creative_attribution(
        &self,
        start_date: NaiveDate,
        end_date: NaiveDate,
        project_id: Option<&str>,
        format: Option<CreativeFormat>,
        hook_type: Option<CreativeHookType>,
    ) -> StorageResult<(
        Vec<CreativeAttributionRow>,
        Vec<CreatorWorkloadSummary>,
        Vec<SeedingBoostAttributionRow>,
    )> {
        let proj = project_id.map(str::to_string);
        self.with_conn(move |conn| {
            let assets = load_creative_assets(conn)?;
            let links = load_creative_links(conn)?;
            let contribs = load_creative_contributions(conn)?;
            let people: HashMap<String, Person> =
                load_people(conn)?.into_iter().map(|p| (p.id.clone(), p)).collect();
            let q = ReportQuery {
                start_date,
                end_date,
                ..Default::default()
            };
            let reports = load_reports_filtered(conn, &q)?;
            Ok(compute_creative_attribution(
                &assets,
                &links,
                &contribs,
                &people,
                &reports,
                proj.as_deref(),
                format.as_ref(),
                hook_type.as_ref(),
            ))
        })
        .await
    }

    async fn upsert_sync_job(&self, job: &SyncJobRecord) -> StorageResult<()> {
        let j = job.clone();
        self.with_conn(move |conn| {
            let plats = to_json(&j.platforms)?;
            conn.execute(
                "INSERT INTO sync_jobs (
                    id, status, platforms_json, start_date, end_date, total_chunks,
                    completed_chunks, failed_chunks, rows_upserted, started_at, finished_at, error_summary
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                    status = excluded.status,
                    total_chunks = excluded.total_chunks,
                    completed_chunks = excluded.completed_chunks,
                    failed_chunks = excluded.failed_chunks,
                    rows_upserted = excluded.rows_upserted,
                    finished_at = excluded.finished_at,
                    error_summary = excluded.error_summary",
                params![
                    j.id,
                    j.status,
                    plats,
                    j.start_date.format("%Y-%m-%d").to_string(),
                    j.end_date.format("%Y-%m-%d").to_string(),
                    j.total_chunks as i64,
                    j.completed_chunks as i64,
                    j.failed_chunks as i64,
                    j.rows_upserted as i64,
                    j.started_at.to_rfc3339(),
                    j.finished_at.map(|t| t.to_rfc3339()),
                    j.error_summary
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_sync_jobs(&self, limit: usize) -> StorageResult<Vec<SyncJobRecord>> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, status, platforms_json, start_date, end_date, total_chunks,
                        completed_chunks, failed_chunks, rows_upserted, started_at, finished_at, error_summary
                 FROM sync_jobs ORDER BY started_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, Option<String>>(11)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, status, plats, sd, ed, tc, cc, fc, ru, sa, fa, es) = row?;
                out.push(SyncJobRecord {
                    id,
                    status,
                    platforms: serde_json::from_str(&plats).unwrap_or_default(),
                    start_date: parse_date(&sd)?,
                    end_date: parse_date(&ed)?,
                    total_chunks: tc.max(0) as usize,
                    completed_chunks: cc.max(0) as usize,
                    failed_chunks: fc.max(0) as usize,
                    rows_upserted: ru.max(0) as usize,
                    started_at: DateTime::parse_from_rfc3339(&sa)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    finished_at: fa.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|d| d.with_timezone(&Utc))
                    }),
                    error_summary: es,
                });
            }
            Ok(out)
        })
        .await
    }

    async fn upsert_sync_checkpoint(&self, cp: &SyncCheckpoint) -> StorageResult<()> {
        let c = cp.clone();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO sync_checkpoints (
                    job_id, platform, ad_account_id, level, chunk_start, chunk_end,
                    status, rows_fetched, error_message, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(job_id, ad_account_id, level, chunk_start, chunk_end) DO UPDATE SET
                    status = excluded.status,
                    rows_fetched = excluded.rows_fetched,
                    error_message = excluded.error_message,
                    updated_at = excluded.updated_at",
                params![
                    c.job_id,
                    c.platform.as_str(),
                    c.ad_account_id,
                    c.level.as_str(),
                    c.chunk_start.format("%Y-%m-%d").to_string(),
                    c.chunk_end.format("%Y-%m-%d").to_string(),
                    c.status,
                    c.rows_fetched as i64,
                    c.error_message,
                    c.updated_at.to_rfc3339()
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_sync_checkpoints(&self, job_id: &str) -> StorageResult<Vec<SyncCheckpoint>> {
        let jid = job_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT job_id, platform, ad_account_id, level, chunk_start, chunk_end,
                        status, rows_fetched, error_message, updated_at
                 FROM sync_checkpoints WHERE job_id = ?1",
            )?;
            let rows = stmt.query_map(params![jid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, String>(9)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (job_id, p, acct, lvl, cs, ce, status, rf, err_msg, ua) = row?;
                out.push(SyncCheckpoint {
                    job_id,
                    platform: p
                        .parse()
                        .map_err(|e: adlink_domain::ParsePlatformError| StorageError::DataIntegrity(e.to_string()))?,
                    ad_account_id: acct,
                    level: parse_enum_str::<ReportLevel>(&lvl)?,
                    chunk_start: parse_date(&cs)?,
                    chunk_end: parse_date(&ce)?,
                    status,
                    rows_fetched: rf.max(0) as usize,
                    error_message: err_msg,
                    updated_at: DateTime::parse_from_rfc3339(&ua)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                });
            }
            Ok(out)
        })
        .await
    }

    async fn append_audit_log(&self, entry: &AuditLogEntry) -> StorageResult<()> {
        let e = entry.clone();
        self.with_conn(move |conn| {
            let ids = to_json(&e.entity_ids)?;
            let detail = to_json(&e.detail)?;
            conn.execute(
                "INSERT INTO audit_logs (
                    id, timestamp, operator, source, platform, ad_account_id,
                    action, entity_ids_json, dry_run, status, reason, detail_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    e.id,
                    e.timestamp.to_rfc3339(),
                    e.operator,
                    e.source,
                    e.platform.as_str(),
                    e.ad_account_id,
                    e.action,
                    ids,
                    if e.dry_run { 1 } else { 0 },
                    e.status,
                    e.reason,
                    detail
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_audit_logs(&self, limit: usize) -> StorageResult<Vec<AuditLogEntry>> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, operator, source, platform, ad_account_id,
                        action, entity_ids_json, dry_run, status, reason, detail_json
                 FROM audit_logs ORDER BY timestamp DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, String>(11)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, ts, op, src, p, acct, act, ids, dry, status, reason, detail) = row?;
                out.push(AuditLogEntry {
                    id,
                    timestamp: DateTime::parse_from_rfc3339(&ts)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    operator: op,
                    source: src,
                    platform: p
                        .parse()
                        .map_err(|e: adlink_domain::ParsePlatformError| StorageError::DataIntegrity(e.to_string()))?,
                    ad_account_id: acct,
                    action: act,
                    entity_ids: serde_json::from_str(&ids).unwrap_or_default(),
                    dry_run: dry != 0,
                    status,
                    reason,
                    detail: serde_json::from_str(&detail).unwrap_or(Value::Null),
                });
            }
            Ok(out)
        })
        .await
    }

    async fn upsert_ad_entities(&self, entities: &[AdEntity]) -> StorageResult<()> {
        let owned = entities.to_vec();
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO ad_entities_cache (
                        platform, ad_account_id, level, entity_id, entity_name,
                        parent_entity_id, status, daily_budget_fen, bid_fen, updated_at, raw_extra_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(platform, ad_account_id, level, entity_id) DO UPDATE SET
                        entity_name = excluded.entity_name,
                        parent_entity_id = excluded.parent_entity_id,
                        status = excluded.status,
                        daily_budget_fen = excluded.daily_budget_fen,
                        bid_fen = excluded.bid_fen,
                        updated_at = excluded.updated_at,
                        raw_extra_json = excluded.raw_extra_json",
                )?;
                for e in &owned {
                    let raw = to_json(&e.raw_extra)?;
                    stmt.execute(params![
                        e.platform.as_str(),
                        e.ad_account_id,
                        e.level.as_str(),
                        e.entity_id,
                        e.entity_name,
                        e.parent_entity_id,
                        e.status,
                        e.daily_budget_fen.as_fen(),
                        e.bid_fen.as_fen(),
                        e.updated_at.to_rfc3339(),
                        raw
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn list_ad_entities(
        &self,
        platform: Option<PlatformId>,
        ad_account_id: Option<&str>,
    ) -> StorageResult<Vec<AdEntity>> {
        let plat = platform.map(|p| p.as_str().to_string());
        let acct = ad_account_id.map(str::to_string);
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT platform, ad_account_id, level, entity_id, entity_name,
                        parent_entity_id, status, daily_budget_fen, bid_fen, updated_at, raw_extra_json
                 FROM ad_entities_cache
                 WHERE (?1 IS NULL OR platform = ?1)
                   AND (?2 IS NULL OR ad_account_id = ?2)
                 ORDER BY platform, ad_account_id, entity_id",
            )?;
            let rows = stmt.query_map(params![plat, acct], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (p, acct_id, lvl, eid, ename, parent, status, budget, bid, ua, raw) = row?;
                out.push(AdEntity {
                    platform: p
                        .parse()
                        .map_err(|e: adlink_domain::ParsePlatformError| StorageError::DataIntegrity(e.to_string()))?,
                    ad_account_id: acct_id,
                    level: parse_enum_str::<ReportLevel>(&lvl)?,
                    entity_id: eid,
                    entity_name: ename,
                    parent_entity_id: parent,
                    status,
                    daily_budget_fen: Fen(budget),
                    bid_fen: Fen(bid),
                    updated_at: DateTime::parse_from_rfc3339(&ua)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    raw_extra: serde_json::from_str(&raw).unwrap_or(Value::Null),
                });
            }
            Ok(out)
        })
        .await
    }

    async fn count_report_rows(&self) -> StorageResult<usize> {
        self.with_conn(|conn| {
            let cnt: Option<i64> = conn
                .query_row("SELECT COUNT(*) FROM unified_reports", [], |r| r.get(0))
                .optional()?;
            Ok(cnt.unwrap_or(0).max(0) as usize)
        })
        .await
    }

    async fn db_health_summary(&self) -> StorageResult<Value> {
        self.with_conn(|conn| {
            configure_connection(conn)?;
            let count = |table: &str| -> i64 {
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap_or(0)
            };
            Ok(serde_json::json!({
                "engine": "sqlite_wal",
                "organizations": count("organizations"),
                "people": count("people"),
                "ad_accounts": count("ad_accounts"),
                "projects": count("projects"),
                "creative_assets": count("creative_assets"),
                "unified_reports": count("unified_reports"),
                "fund_statements": count("daily_fund_statements"),
                "sync_jobs": count("sync_jobs"),
                "audit_logs": count("audit_logs"),
            }))
        })
        .await
    }
}

fn load_people(conn: &Connection) -> StorageResult<Vec<Person>> {
    let mut stmt = conn.prepare(
        "SELECT id, org_id, name, email, department, role, monthly_cost_fen, active
         FROM people ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, i64>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, org_id, name, email, dept, role, mcf, act) = row?;
        out.push(Person {
            id,
            org_id,
            name,
            email,
            department: dept,
            role: parse_enum_str::<PersonRole>(&role)?,
            monthly_cost_fen: Fen(mcf),
            active: act != 0,
        });
    }
    Ok(out)
}

fn load_accounts(conn: &Connection) -> StorageResult<Vec<AdAccount>> {
    let mut stmt = conn.prepare(
        "SELECT id, org_id, platform, platform_account_id, account_name, account_type,
                rebate_rate, service_fee_rate, default_cash_ratio, status, credential_ref, extra_json
         FROM ad_accounts ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, f64>(6)?,
            r.get::<_, f64>(7)?,
            r.get::<_, f64>(8)?,
            r.get::<_, String>(9)?,
            r.get::<_, Option<String>>(10)?,
            r.get::<_, String>(11)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, org_id, plat, pat_id, name, at, rr, sfr, dcr, status, cref, extra) = row?;
        out.push(AdAccount {
            id,
            org_id,
            platform: plat
                .parse()
                .map_err(|e: adlink_domain::ParsePlatformError| StorageError::DataIntegrity(e.to_string()))?,
            platform_account_id: pat_id,
            account_name: name,
            account_type: parse_enum_str::<AccountType>(&at)?,
            rebate_rate: rr,
            service_fee_rate: sfr,
            default_cash_ratio: dcr,
            status,
            credential_ref: cref,
            extra: serde_json::from_str(&extra).unwrap_or(Value::Null),
        });
    }
    Ok(out)
}

fn load_bindings(conn: &Connection) -> StorageResult<Vec<PersonAccountBinding>> {
    let mut stmt = conn.prepare(
        "SELECT id, person_id, ad_account_id, relation_role, split_weight, effective_from, effective_to
         FROM person_account_bindings ORDER BY effective_from, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, f64>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, pid, aid, role, weight, ef, et) = row?;
        out.push(PersonAccountBinding {
            id,
            person_id: pid,
            ad_account_id: aid,
            relation_role: parse_enum_str::<BindingRole>(&role)?,
            split_weight: weight,
            effective_from: parse_date(&ef)?,
            effective_to: et.as_deref().map(parse_date).transpose()?,
        });
    }
    Ok(out)
}

fn load_projects(conn: &Connection) -> StorageResult<Vec<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, org_id, code, name, client_name, brand_name, project_type,
                business_line, settlement_mode, budget_fen, target_roi, target_cpa_fen,
                owner_person_id, start_date, end_date, tags_json
         FROM projects ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, i64>(9)?,
            r.get::<_, Option<f64>>(10)?,
            r.get::<_, Option<i64>>(11)?,
            r.get::<_, Option<String>>(12)?,
            r.get::<_, String>(13)?,
            r.get::<_, Option<String>>(14)?,
            r.get::<_, String>(15)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, oid, code, name, cn, bn, pt, bl, sm, bf, troi, tcpa, opid, sd, ed, tags) = row?;
        out.push(Project {
            id,
            org_id: oid,
            code,
            name,
            client_name: cn,
            brand_name: bn,
            project_type: parse_enum_str::<ProjectType>(&pt)?,
            business_line: bl,
            settlement_mode: parse_enum_str::<SettlementMode>(&sm)?,
            budget_fen: Fen(bf),
            target_roi: troi,
            target_cpa_fen: tcpa.map(Fen),
            owner_person_id: opid,
            start_date: parse_date(&sd)?,
            end_date: ed.as_deref().map(parse_date).transpose()?,
            tags: serde_json::from_str(&tags).unwrap_or_default(),
        });
    }
    Ok(out)
}

fn load_project_links(conn: &Connection) -> StorageResult<Vec<ProjectAccountLink>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, ad_account_id, campaign_filter_regex, allocation_weight
         FROM project_account_links ORDER BY project_id, ad_account_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ProjectAccountLink {
            id: r.get(0)?,
            project_id: r.get(1)?,
            ad_account_id: r.get(2)?,
            campaign_filter_regex: r.get(3)?,
            allocation_weight: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn load_creative_assets(conn: &Connection) -> StorageResult<Vec<CreativeAsset>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, asset_code, title, format, hook_type, scene_tag,
                selling_point, tone, custom_tags_json, production_cost_fen, production_hours,
                created_date, status
         FROM creative_assets ORDER BY created_date DESC, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, String>(9)?,
            r.get::<_, i64>(10)?,
            r.get::<_, f64>(11)?,
            r.get::<_, String>(12)?,
            r.get::<_, String>(13)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, pid, code, title, fmt, hook, scene, sp, tone, tags, pcf, ph, cd, st) = row?;
        out.push(CreativeAsset {
            id,
            project_id: pid,
            asset_code: code,
            title,
            taxonomy: CreativeTaxonomy {
                format: parse_enum_str::<CreativeFormat>(&fmt)?,
                hook_type: parse_enum_str::<CreativeHookType>(&hook)?,
                scene_tag: scene,
                selling_point: sp,
                tone,
                custom_tags: serde_json::from_str::<BTreeMap<String, String>>(&tags)
                    .unwrap_or_default(),
            },
            production_cost_fen: Fen(pcf),
            production_hours: ph,
            created_date: parse_date(&cd)?,
            status: st,
        });
    }
    Ok(out)
}

fn load_creative_links(conn: &Connection) -> StorageResult<Vec<CreativePlatformLink>> {
    let mut stmt = conn.prepare(
        "SELECT id, asset_id, platform, ad_account_id, platform_creative_id, platform_content_id
         FROM creative_platform_links ORDER BY asset_id, platform",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, asset_id, p, aid, pcid, content_id) = row?;
        out.push(CreativePlatformLink {
            id,
            asset_id,
            platform: p
                .parse()
                .map_err(|e: adlink_domain::ParsePlatformError| StorageError::DataIntegrity(e.to_string()))?,
            ad_account_id: aid,
            platform_creative_id: pcid,
            platform_content_id: content_id,
        });
    }
    Ok(out)
}

fn load_creative_contributions(conn: &Connection) -> StorageResult<Vec<CreativeProductionContribution>> {
    let mut stmt = conn.prepare(
        "SELECT id, asset_id, person_id, role, work_hours, credit_weight, piece_cost_fen
         FROM creative_production_contributions ORDER BY asset_id, person_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, f64>(4)?,
            r.get::<_, f64>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, aid, pid, role, wh, cw, pcf) = row?;
        out.push(CreativeProductionContribution {
            id,
            asset_id: aid,
            person_id: pid,
            role: parse_enum_str::<ProductionRole>(&role)?,
            work_hours: wh,
            credit_weight: cw,
            piece_cost_fen: Fen(pcf),
        });
    }
    Ok(out)
}

fn load_custom_calibers(conn: &Connection) -> StorageResult<Vec<CustomCaliberDef>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, display_name, category, formula, unit, description, enabled
         FROM custom_calibers ORDER BY name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, i64>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, dn, cat, formula, unit, desc, en) = row?;
        out.push(CustomCaliberDef {
            id,
            name,
            display_name: dn,
            category: parse_enum_str::<CaliberCategory>(&cat)?,
            formula,
            unit,
            description: desc,
            enabled: en != 0,
        });
    }
    Ok(out)
}

fn load_universe(
    conn: &Connection,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> StorageResult<AttributionUniverse> {
    let mut stmt = conn.prepare(
        "SELECT ad_account_id, stat_date, cash_cost_fen, grant_cost_fen
         FROM daily_fund_statements
         WHERE stat_date >= ?1 AND stat_date <= ?2",
    )?;
    let start_s = start_date.format("%Y-%m-%d").to_string();
    let end_s = end_date.format("%Y-%m-%d").to_string();
    let fund_rows = stmt.query_map(params![start_s, end_s], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut fund_cash_ratios = HashMap::new();
    for item in fund_rows {
        let (acct_id, dt_s, cc, gc) = item?;
        let total = cc + gc;
        if total > 0 {
            if let Ok(dt) = parse_date(&dt_s) {
                fund_cash_ratios.insert((acct_id, dt), (cc as f64) / (total as f64));
            }
        }
    }

    Ok(AttributionUniverse {
        accounts: load_accounts(conn)?
            .into_iter()
            .map(|a| (a.id.clone(), a))
            .collect(),
        people: load_people(conn)?
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect(),
        bindings: load_bindings(conn)?,
        projects: load_projects(conn)?
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect(),
        project_links: load_project_links(conn)?,
        custom_calibers: load_custom_calibers(conn)?,
        fund_cash_ratios,
    })
}

fn load_reports_filtered(
    conn: &Connection,
    query: &ReportQuery,
) -> StorageResult<Vec<UnifiedReportRow>> {
    let mut stmt = conn.prepare(
        "SELECT platform, ad_account_id, platform_account_id, stat_date, stat_hour,
                granularity, level, entity_id, entity_name, parent_entity_id, content_id,
                time_line, gross_cost_fen, cash_cost_fen, grant_cost_fen,
                kol_order_fee_fen, platform_service_fee_fen, impressions, clicks,
                conversions, deep_conversions, leads, orders, gmv_fen, interactions,
                video_plays, effective_plays, new_follows, store_verifies, raw_fields_json
         FROM unified_reports
         WHERE stat_date >= ?1 AND stat_date <= ?2
         ORDER BY stat_date ASC, platform ASC, entity_id ASC",
    )?;
    let start_s = query.start_date.format("%Y-%m-%d").to_string();
    let end_s = query.end_date.format("%Y-%m-%d").to_string();

    let rows = stmt.query_map(params![start_s, end_s], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i32>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, Option<String>>(9)?,
            r.get::<_, Option<String>>(10)?,
            r.get::<_, String>(11)?,
            r.get::<_, i64>(12)?,
            r.get::<_, i64>(13)?,
            r.get::<_, i64>(14)?,
            r.get::<_, i64>(15)?,
            r.get::<_, i64>(16)?,
            r.get::<_, i64>(17)?,
            r.get::<_, i64>(18)?,
            r.get::<_, i64>(19)?,
            r.get::<_, i64>(20)?,
            r.get::<_, i64>(21)?,
            r.get::<_, i64>(22)?,
            r.get::<_, i64>(23)?,
            r.get::<_, i64>(24)?,
            r.get::<_, i64>(25)?,
            r.get::<_, i64>(26)?,
            r.get::<_, i64>(27)?,
            r.get::<_, i64>(28)?,
            r.get::<_, String>(29)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (
            plat,
            acct,
            pacct,
            sd,
            sh,
            gran,
            lvl,
            eid,
            ename,
            peid,
            cid,
            tl,
            gross,
            cash,
            grant,
            kol,
            svc,
            imp,
            clk,
            conv,
            dconv,
            leads,
            ord,
            gmv,
            inter,
            vplay,
            eplay,
            nfol,
            sver,
            raw,
        ) = row?;

        let platform: PlatformId = plat
            .parse()
            .map_err(|e: adlink_domain::ParsePlatformError| StorageError::DataIntegrity(e.to_string()))?;
        if !query.platforms.is_empty() && !query.platforms.contains(&platform) {
            continue;
        }
        if !query.ad_account_ids.is_empty() && !query.ad_account_ids.contains(&acct) {
            continue;
        }
        let level = parse_enum_str::<ReportLevel>(&lvl)?;
        if !query.levels.is_empty() && !query.levels.contains(&level) {
            continue;
        }

        out.push(UnifiedReportRow {
            platform,
            ad_account_id: acct,
            platform_account_id: pacct,
            stat_date: parse_date(&sd)?,
            stat_hour: sh,
            granularity: parse_enum_str::<ReportGranularity>(&gran)?,
            level,
            entity_id: eid,
            entity_name: ename,
            parent_entity_id: peid,
            content_id: cid,
            time_line: parse_enum_str::<TimeLine>(&tl)?,
            gross_cost_fen: Fen(gross),
            cash_cost_fen: Fen(cash),
            grant_cost_fen: Fen(grant),
            kol_order_fee_fen: Fen(kol),
            platform_service_fee_fen: Fen(svc),
            impressions: imp.max(0) as u64,
            clicks: clk.max(0) as u64,
            conversions: conv.max(0) as u64,
            deep_conversions: dconv.max(0) as u64,
            leads: leads.max(0) as u64,
            orders: ord.max(0) as u64,
            gmv_fen: Fen(gmv),
            interactions: inter.max(0) as u64,
            video_plays: vplay.max(0) as u64,
            effective_plays: eplay.max(0) as u64,
            new_follows: nfol.max(0) as u64,
            store_verifies: sver.max(0) as u64,
            raw_fields: serde_json::from_str(&raw).unwrap_or_default(),
        });
    }
    Ok(out)
}
