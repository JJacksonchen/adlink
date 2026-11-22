//! SQLite schema DDL and idempotent migrations for AdLink governance, metrics, and checkpoints.

use rusqlite::Connection;

use crate::StorageError;

pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    org_type TEXT NOT NULL,
    default_currency TEXT NOT NULL DEFAULT 'CNY',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS people (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    email TEXT,
    department TEXT NOT NULL,
    role TEXT NOT NULL,
    monthly_cost_fen INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS ad_accounts (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    platform TEXT NOT NULL,
    platform_account_id TEXT NOT NULL,
    account_name TEXT NOT NULL,
    account_type TEXT NOT NULL,
    rebate_rate REAL NOT NULL DEFAULT 0.0,
    service_fee_rate REAL NOT NULL DEFAULT 0.0,
    default_cash_ratio REAL NOT NULL DEFAULT 1.0,
    status TEXT NOT NULL DEFAULT 'active',
    credential_ref TEXT,
    extra_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(platform, platform_account_id)
);

CREATE TABLE IF NOT EXISTS person_account_bindings (
    id TEXT PRIMARY KEY,
    person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    ad_account_id TEXT NOT NULL REFERENCES ad_accounts(id) ON DELETE CASCADE,
    relation_role TEXT NOT NULL,
    split_weight REAL NOT NULL DEFAULT 1.0,
    effective_from TEXT NOT NULL,
    effective_to TEXT
);
CREATE INDEX IF NOT EXISTS idx_pab_account_dates
    ON person_account_bindings(ad_account_id, effective_from, effective_to);
CREATE INDEX IF NOT EXISTS idx_pab_person
    ON person_account_bindings(person_id);

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    code TEXT NOT NULL,
    name TEXT NOT NULL,
    client_name TEXT NOT NULL,
    brand_name TEXT NOT NULL,
    project_type TEXT NOT NULL,
    business_line TEXT NOT NULL,
    settlement_mode TEXT NOT NULL,
    budget_fen INTEGER NOT NULL DEFAULT 0,
    target_roi REAL,
    target_cpa_fen INTEGER,
    owner_person_id TEXT REFERENCES people(id),
    start_date TEXT NOT NULL,
    end_date TEXT,
    tags_json TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS project_account_links (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    ad_account_id TEXT NOT NULL REFERENCES ad_accounts(id) ON DELETE CASCADE,
    campaign_filter_regex TEXT,
    allocation_weight REAL NOT NULL DEFAULT 1.0,
    UNIQUE(project_id, ad_account_id)
);

CREATE TABLE IF NOT EXISTS creative_assets (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    asset_code TEXT NOT NULL,
    title TEXT NOT NULL,
    format TEXT NOT NULL,
    hook_type TEXT NOT NULL,
    scene_tag TEXT NOT NULL,
    selling_point TEXT NOT NULL,
    tone TEXT NOT NULL,
    custom_tags_json TEXT NOT NULL DEFAULT '{}',
    production_cost_fen INTEGER NOT NULL DEFAULT 0,
    production_hours REAL NOT NULL DEFAULT 0.0,
    created_date TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active'
);

CREATE TABLE IF NOT EXISTS creative_platform_links (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL REFERENCES creative_assets(id) ON DELETE CASCADE,
    platform TEXT NOT NULL,
    ad_account_id TEXT NOT NULL REFERENCES ad_accounts(id) ON DELETE CASCADE,
    platform_creative_id TEXT NOT NULL,
    platform_content_id TEXT,
    UNIQUE(platform, ad_account_id, platform_creative_id)
);
CREATE INDEX IF NOT EXISTS idx_cpl_content
    ON creative_platform_links(platform_content_id);
CREATE INDEX IF NOT EXISTS idx_cpl_asset
    ON creative_platform_links(asset_id);

CREATE TABLE IF NOT EXISTS creative_production_contributions (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL REFERENCES creative_assets(id) ON DELETE CASCADE,
    person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    work_hours REAL NOT NULL DEFAULT 0.0,
    credit_weight REAL NOT NULL DEFAULT 1.0,
    piece_cost_fen INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_cpc_asset ON creative_production_contributions(asset_id);
CREATE INDEX IF NOT EXISTS idx_cpc_person ON creative_production_contributions(person_id);

CREATE TABLE IF NOT EXISTS custom_calibers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    category TEXT NOT NULL,
    formula TEXT NOT NULL,
    unit TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS unified_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform TEXT NOT NULL,
    ad_account_id TEXT NOT NULL,
    platform_account_id TEXT NOT NULL,
    stat_date TEXT NOT NULL,
    stat_hour INTEGER NOT NULL DEFAULT -1,
    granularity TEXT NOT NULL,
    level TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    entity_name TEXT NOT NULL,
    parent_entity_id TEXT,
    content_id TEXT,
    time_line TEXT NOT NULL,
    gross_cost_fen INTEGER NOT NULL DEFAULT 0,
    cash_cost_fen INTEGER NOT NULL DEFAULT 0,
    grant_cost_fen INTEGER NOT NULL DEFAULT 0,
    kol_order_fee_fen INTEGER NOT NULL DEFAULT 0,
    platform_service_fee_fen INTEGER NOT NULL DEFAULT 0,
    impressions INTEGER NOT NULL DEFAULT 0,
    clicks INTEGER NOT NULL DEFAULT 0,
    conversions INTEGER NOT NULL DEFAULT 0,
    deep_conversions INTEGER NOT NULL DEFAULT 0,
    leads INTEGER NOT NULL DEFAULT 0,
    orders INTEGER NOT NULL DEFAULT 0,
    gmv_fen INTEGER NOT NULL DEFAULT 0,
    interactions INTEGER NOT NULL DEFAULT 0,
    video_plays INTEGER NOT NULL DEFAULT 0,
    effective_plays INTEGER NOT NULL DEFAULT 0,
    new_follows INTEGER NOT NULL DEFAULT 0,
    store_verifies INTEGER NOT NULL DEFAULT 0,
    raw_fields_json TEXT NOT NULL DEFAULT '{}',
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform, ad_account_id, stat_date, stat_hour, granularity, level, entity_id, time_line)
);
CREATE INDEX IF NOT EXISTS idx_ur_date_platform ON unified_reports(stat_date, platform);
CREATE INDEX IF NOT EXISTS idx_ur_account_date ON unified_reports(ad_account_id, stat_date);
CREATE INDEX IF NOT EXISTS idx_ur_entity ON unified_reports(entity_id);
CREATE INDEX IF NOT EXISTS idx_ur_content ON unified_reports(content_id);

CREATE TABLE IF NOT EXISTS daily_fund_statements (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform TEXT NOT NULL,
    ad_account_id TEXT NOT NULL,
    platform_account_id TEXT NOT NULL,
    stat_date TEXT NOT NULL,
    cash_balance_fen INTEGER NOT NULL DEFAULT 0,
    grant_balance_fen INTEGER NOT NULL DEFAULT 0,
    cash_cost_fen INTEGER NOT NULL DEFAULT 0,
    grant_cost_fen INTEGER NOT NULL DEFAULT 0,
    shared_wallet_cost_fen INTEGER NOT NULL DEFAULT 0,
    income_fen INTEGER NOT NULL DEFAULT 0,
    raw_fields_json TEXT NOT NULL DEFAULT '{}',
    UNIQUE(platform, ad_account_id, stat_date)
);
CREATE INDEX IF NOT EXISTS idx_dfs_account_date ON daily_fund_statements(ad_account_id, stat_date);

CREATE TABLE IF NOT EXISTS sync_jobs (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    platforms_json TEXT NOT NULL,
    start_date TEXT NOT NULL,
    end_date TEXT NOT NULL,
    total_chunks INTEGER NOT NULL DEFAULT 0,
    completed_chunks INTEGER NOT NULL DEFAULT 0,
    failed_chunks INTEGER NOT NULL DEFAULT 0,
    rows_upserted INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    error_summary TEXT
);

CREATE TABLE IF NOT EXISTS sync_checkpoints (
    job_id TEXT NOT NULL,
    platform TEXT NOT NULL,
    ad_account_id TEXT NOT NULL,
    level TEXT NOT NULL,
    chunk_start TEXT NOT NULL,
    chunk_end TEXT NOT NULL,
    status TEXT NOT NULL,
    rows_fetched INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(job_id, ad_account_id, level, chunk_start, chunk_end)
);

CREATE TABLE IF NOT EXISTS audit_logs (
    id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL,
    operator TEXT NOT NULL,
    source TEXT NOT NULL,
    platform TEXT NOT NULL,
    ad_account_id TEXT NOT NULL,
    action TEXT NOT NULL,
    entity_ids_json TEXT NOT NULL,
    dry_run INTEGER NOT NULL,
    status TEXT NOT NULL,
    reason TEXT NOT NULL,
    detail_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_logs(timestamp DESC);

CREATE TABLE IF NOT EXISTS ad_entities_cache (
    platform TEXT NOT NULL,
    ad_account_id TEXT NOT NULL,
    level TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    entity_name TEXT NOT NULL,
    parent_entity_id TEXT,
    status TEXT NOT NULL,
    daily_budget_fen INTEGER NOT NULL DEFAULT 0,
    bid_fen INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    raw_extra_json TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY(platform, ad_account_id, level, entity_id)
);
"#;

/// Applies pragmas optimized for concurrent reads and durable transactional batch writes.
pub fn configure_connection(conn: &Connection) -> Result<(), StorageError> {
    configure_writer_connection(conn)
}

/// Applies writer pragmas: WAL journal, generous busy timeout, memory temp store,
/// 16 MiB page cache, and periodic WAL autocheckpoint.
pub fn configure_writer_connection(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 10000;
        PRAGMA temp_store = MEMORY;
        PRAGMA cache_size = -16384;
        PRAGMA wal_autocheckpoint = 1000;
        ",
    )?;
    Ok(())
}

/// Applies read-only pool pragmas (`query_only = ON`, mmap I/O, 16 MiB cache).
pub fn configure_reader_connection(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        "
        PRAGMA query_only = ON;
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 10000;
        PRAGMA temp_store = MEMORY;
        PRAGMA cache_size = -16384;
        PRAGMA mmap_size = 268435456;
        ",
    )?;
    Ok(())
}

/// Runs idempotent schema creation on the database connection.
pub fn run_migrations(conn: &Connection) -> Result<(), StorageError> {
    configure_writer_connection(conn)?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(())
}
