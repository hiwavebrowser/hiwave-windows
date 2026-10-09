use chrono::{DateTime, Utc};
use hiwave_core::error::{HiWaveError, HiWaveResult};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

use crate::types::{AnalyticsEvent, DailyStats, DomainStats, ReportSettings, WorkspaceStats};

/// Database schema version for migrations
const SCHEMA_VERSION: i32 = 2;

/// Initialize or open the analytics database
pub fn init_database(path: &Path) -> HiWaveResult<Connection> {
    let conn = Connection::open(path).map_err(|e| {
        HiWaveError::analytics(format!("Failed to open analytics database: {}", e))
    })?;

    // Enable foreign keys
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    // Check if schema_version table exists
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get::<_, i32>(0).map(|count| count > 0),
        )
        .unwrap_or(false);

    let version: i32 = if table_exists {
        conn.query_row(
            "SELECT version FROM schema_version ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| HiWaveError::analytics(e.to_string()))?
        .unwrap_or(0)
    } else {
        0
    };

    if version < SCHEMA_VERSION {
        migrate_database(&conn, version)?;
    }

    Ok(conn)
}

/// Run database migrations
fn migrate_database(conn: &Connection, from_version: i32) -> HiWaveResult<()> {
    if from_version < 1 {
        create_schema_v1(conn)?;
    }

    if from_version < 2 {
        migrate_to_v2(conn)?;
    }

    Ok(())
}

/// Create initial schema (version 1)
fn create_schema_v1(conn: &Connection) -> HiWaveResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_version (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL,
            applied_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS analytics_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_type TEXT NOT NULL,
            domain TEXT,
            details TEXT,
            workspace_id TEXT,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_events_type ON analytics_events(event_type);
        CREATE INDEX IF NOT EXISTS idx_events_domain ON analytics_events(domain);
        CREATE INDEX IF NOT EXISTS idx_events_created ON analytics_events(created_at);
        CREATE INDEX IF NOT EXISTS idx_events_workspace ON analytics_events(workspace_id);

        CREATE TABLE IF NOT EXISTS daily_stats (
            date TEXT PRIMARY KEY,
            trackers_blocked INTEGER DEFAULT 0,
            ads_blocked INTEGER DEFAULT 0,
            popups_blocked INTEGER DEFAULT 0,
            pages_visited INTEGER DEFAULT 0,
            tabs_opened INTEGER DEFAULT 0,
            tabs_closed INTEGER DEFAULT 0,
            browsing_time INTEGER DEFAULT 0,
            focus_time INTEGER DEFAULT 0,
            workspace_switches INTEGER DEFAULT 0,
            time_saved INTEGER DEFAULT 0,
            bandwidth_saved INTEGER DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS domain_stats (
            domain TEXT PRIMARY KEY,
            visit_count INTEGER DEFAULT 0,
            total_time INTEGER DEFAULT 0,
            trackers_blocked INTEGER DEFAULT 0,
            last_visit INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS workspace_stats (
            workspace_id TEXT PRIMARY KEY,
            total_time INTEGER DEFAULT 0,
            tab_count INTEGER DEFAULT 0,
            visit_count INTEGER DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS report_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            enabled INTEGER NOT NULL DEFAULT 1,
            retention_days INTEGER NOT NULL DEFAULT 90,
            weekly_report INTEGER NOT NULL DEFAULT 1,
            report_day TEXT NOT NULL DEFAULT 'Monday'
        );

        -- Insert default settings
        INSERT OR IGNORE INTO report_settings (id, enabled, retention_days, weekly_report, report_day)
        VALUES (1, 1, 90, 1, 'Monday');
        "#,
    )
    .map_err(|e| HiWaveError::analytics(format!("Failed to create schema: {}", e)))?;

    conn.execute(
        "INSERT INTO schema_version (version, applied_at) VALUES (?, ?)",
        params![SCHEMA_VERSION, Utc::now().timestamp()],
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(())
}

/// Check if a column exists in a table.
fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let query = format!("PRAGMA table_info({})", table);
    match conn.prepare(&query) {
        Ok(mut stmt) => {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1));
            if let Ok(rows) = rows {
                for col in rows.flatten() {
                    if col == column {
                        return true;
                    }
                }
            }
            false
        }
        Err(_) => false,
    }
}

/// Migrate to schema version 2 (add time_saved and bandwidth_saved)
fn migrate_to_v2(conn: &Connection) -> HiWaveResult<()> {
    // Check if columns already exist (they may be in the initial schema)
    let has_time_saved = column_exists(conn, "daily_stats", "time_saved");
    let has_bandwidth_saved = column_exists(conn, "daily_stats", "bandwidth_saved");
    
    // Add new columns to daily_stats table if they don't exist
    if !has_time_saved {
        conn.execute(
            "ALTER TABLE daily_stats ADD COLUMN time_saved INTEGER DEFAULT 0",
            [],
        )
        .map_err(|e| HiWaveError::analytics(format!("Failed to add time_saved column: {}", e)))?;
    }

    if !has_bandwidth_saved {
        conn.execute(
            "ALTER TABLE daily_stats ADD COLUMN bandwidth_saved INTEGER DEFAULT 0",
            [],
        )
        .map_err(|e| HiWaveError::analytics(format!("Failed to add bandwidth_saved column: {}", e)))?;
    }

    // Add columns to archive table if it exists
    let archive_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='daily_stats_archive'",
            [],
            |row| row.get::<_, i32>(0).map(|count| count > 0),
        )
        .unwrap_or(false);

    if archive_exists {
        let _ = conn.execute(
            "ALTER TABLE daily_stats_archive ADD COLUMN time_saved INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE daily_stats_archive ADD COLUMN bandwidth_saved INTEGER DEFAULT 0",
            [],
        );
    }

    // Record migration
    conn.execute(
        "INSERT INTO schema_version (version, applied_at) VALUES (?, ?)",
        params![2, Utc::now().timestamp()],
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(())
}

/// Insert an analytics event
pub fn insert_event(
    conn: &Connection,
    event: &AnalyticsEvent,
    workspace_id: Option<&str>,
) -> HiWaveResult<()> {
    let (event_type, domain, details) = match event {
        AnalyticsEvent::TrackerBlocked { domain } => {
            ("tracker_blocked", Some(domain.as_str()), None)
        }
        AnalyticsEvent::AdBlocked { domain } => ("ad_blocked", Some(domain.as_str()), None),
        AnalyticsEvent::PopupBlocked { domain } => ("popup_blocked", Some(domain.as_str()), None),
        AnalyticsEvent::PageVisit { domain } => ("page_visit", Some(domain.as_str()), None),
        AnalyticsEvent::TabOpened => ("tab_opened", None, None),
        AnalyticsEvent::TabClosed { duration_secs } => (
            "tab_closed",
            None,
            Some(serde_json::json!({ "duration_secs": duration_secs }).to_string()),
        ),
        AnalyticsEvent::TabToShelf { domain } => ("tab_to_shelf", Some(domain.as_str()), None),
        AnalyticsEvent::TabFromShelf { domain } => {
            ("tab_from_shelf", Some(domain.as_str()), None)
        }
        AnalyticsEvent::WorkspaceSwitch { from, to } => (
            "workspace_switch",
            None,
            Some(serde_json::json!({ "from": from, "to": to }).to_string()),
        ),
        AnalyticsEvent::FocusStart { domain } => ("focus_start", Some(domain.as_str()), None),
        AnalyticsEvent::FocusEnd {
            domain,
            duration_secs,
        } => (
            "focus_end",
            Some(domain.as_str()),
            Some(serde_json::json!({ "duration_secs": duration_secs }).to_string()),
        ),
        AnalyticsEvent::SessionStart => ("session_start", None, None),
        AnalyticsEvent::SessionEnd { duration_secs } => (
            "session_end",
            None,
            Some(serde_json::json!({ "duration_secs": duration_secs }).to_string()),
        ),
    };

    conn.execute(
        "INSERT INTO analytics_events (event_type, domain, details, workspace_id, created_at) VALUES (?, ?, ?, ?, ?)",
        params![
            event_type,
            domain,
            details,
            workspace_id,
            Utc::now().timestamp()
        ],
    )
    .map_err(|e| HiWaveError::analytics(format!("Failed to insert event: {}", e)))?;

    Ok(())
}

/// Get daily stats for a specific date
pub fn get_daily_stats(conn: &Connection, date: &str) -> HiWaveResult<DailyStats> {
    conn.query_row(
        "SELECT date, trackers_blocked, ads_blocked, popups_blocked, pages_visited,
                tabs_opened, tabs_closed, browsing_time, focus_time, workspace_switches,
                time_saved, bandwidth_saved
         FROM daily_stats WHERE date = ?",
        [date],
        |row| {
            Ok(DailyStats {
                date: row.get(0)?,
                trackers_blocked: row.get(1)?,
                ads_blocked: row.get(2)?,
                popups_blocked: row.get(3)?,
                pages_visited: row.get(4)?,
                tabs_opened: row.get(5)?,
                tabs_closed: row.get(6)?,
                browsing_time: row.get(7)?,
                focus_time: row.get(8)?,
                workspace_switches: row.get(9)?,
                time_saved: row.get(10)?,
                bandwidth_saved: row.get(11)?,
            })
        },
    )
    .optional()
    .map_err(|e| HiWaveError::analytics(e.to_string()))?
    .ok_or_else(|| HiWaveError::NotFound(format!("No stats for date: {}", date)))
}

/// Get or create daily stats for today
pub fn get_or_create_today_stats(conn: &Connection) -> HiWaveResult<DailyStats> {
    let today = Utc::now().format("%Y-%m-%d").to_string();

    // Try to get existing
    if let Ok(stats) = get_daily_stats(conn, &today) {
        return Ok(stats);
    }

    // Create new entry
    conn.execute(
        "INSERT INTO daily_stats (date) VALUES (?)",
        [&today],
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    get_daily_stats(conn, &today)
}

/// Increment a daily stat counter
pub fn increment_daily_stat(conn: &Connection, date: &str, field: &str) -> HiWaveResult<()> {
    let query = format!(
        "UPDATE daily_stats SET {} = {} + 1 WHERE date = ?",
        field, field
    );
    conn.execute(&query, [date])
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;
    Ok(())
}

/// Add time to a daily stat field
pub fn add_daily_time(conn: &Connection, date: &str, field: &str, seconds: i64) -> HiWaveResult<()> {
    let query = format!(
        "UPDATE daily_stats SET {} = {} + ? WHERE date = ?",
        field, field
    );
    conn.execute(&query, params![seconds, date])
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;
    Ok(())
}

/// Get report settings
pub fn get_settings(conn: &Connection) -> HiWaveResult<ReportSettings> {
    conn.query_row(
        "SELECT enabled, retention_days, weekly_report, report_day FROM report_settings WHERE id = 1",
        [],
        |row| {
            Ok(ReportSettings {
                enabled: row.get::<_, i32>(0)? == 1,
                retention_days: row.get(1)?,
                weekly_report: row.get::<_, i32>(2)? == 1,
                report_day: row.get(3)?,
            })
        },
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))
}

/// Update report settings
pub fn update_settings(conn: &Connection, settings: &ReportSettings) -> HiWaveResult<()> {
    conn.execute(
        "UPDATE report_settings SET enabled = ?, retention_days = ?, weekly_report = ?, report_day = ? WHERE id = 1",
        params![
            if settings.enabled { 1 } else { 0 },
            settings.retention_days,
            if settings.weekly_report { 1 } else { 0 },
            settings.report_day
        ],
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))?;
    Ok(())
}

/// Delete events older than retention period
pub fn cleanup_old_events(conn: &Connection, retention_days: i32) -> HiWaveResult<usize> {
    let cutoff = Utc::now().timestamp() - (retention_days as i64 * 86400);

    let deleted = conn
        .execute(
            "DELETE FROM analytics_events WHERE created_at < ?",
            [cutoff],
        )
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(deleted)
}

/// Get top domains by visit count
pub fn get_top_domains(conn: &Connection, limit: usize) -> HiWaveResult<Vec<DomainStats>> {
    let mut stmt = conn
        .prepare(
            "SELECT domain, visit_count, total_time, trackers_blocked, last_visit
             FROM domain_stats
             ORDER BY visit_count DESC
             LIMIT ?",
        )
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    let domains = stmt
        .query_map([limit], |row| {
            Ok(DomainStats {
                domain: row.get(0)?,
                visit_count: row.get(1)?,
                total_time: row.get(2)?,
                trackers_blocked: row.get(3)?,
                last_visit: DateTime::from_timestamp(row.get::<_, i64>(4)?, 0)
                    .unwrap_or_else(Utc::now),
            })
        })
        .map_err(|e| HiWaveError::analytics(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(domains)
}

/// Get workspace statistics
pub fn get_workspace_stats(conn: &Connection) -> HiWaveResult<Vec<WorkspaceStats>> {
    let mut stmt = conn
        .prepare(
            "SELECT workspace_id, total_time, tab_count, visit_count
             FROM workspace_stats
             ORDER BY total_time DESC",
        )
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    let workspaces = stmt
        .query_map([], |row| {
            Ok(WorkspaceStats {
                workspace_id: row.get(0)?,
                total_time: row.get(1)?,
                tab_count: row.get(2)?,
                visit_count: row.get(3)?,
            })
        })
        .map_err(|e| HiWaveError::analytics(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(workspaces)
}

/// Get daily stats for a date range
pub fn get_daily_stats_range(
    conn: &Connection,
    start_date: &str,
    end_date: &str,
) -> HiWaveResult<Vec<DailyStats>> {
    let mut stmt = conn
        .prepare(
            "SELECT date, trackers_blocked, ads_blocked, popups_blocked, pages_visited,
                    tabs_opened, tabs_closed, browsing_time, focus_time, workspace_switches,
                    time_saved, bandwidth_saved
             FROM daily_stats
             WHERE date >= ? AND date <= ?
             ORDER BY date ASC",
        )
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    let stats = stmt
        .query_map([start_date, end_date], |row| {
            Ok(DailyStats {
                date: row.get(0)?,
                trackers_blocked: row.get(1)?,
                ads_blocked: row.get(2)?,
                popups_blocked: row.get(3)?,
                pages_visited: row.get(4)?,
                tabs_opened: row.get(5)?,
                tabs_closed: row.get(6)?,
                browsing_time: row.get(7)?,
                focus_time: row.get(8)?,
                workspace_switches: row.get(9)?,
                time_saved: row.get(10)?,
                bandwidth_saved: row.get(11)?,
            })
        })
        .map_err(|e| HiWaveError::analytics(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(stats)
}

/// Aggregate stats across a date range
pub fn aggregate_stats_range(
    conn: &Connection,
    start_date: &str,
    end_date: &str,
) -> HiWaveResult<DailyStats> {
    let row = conn
        .query_row(
            "SELECT
                SUM(trackers_blocked),
                SUM(ads_blocked),
                SUM(popups_blocked),
                SUM(pages_visited),
                SUM(tabs_opened),
                SUM(tabs_closed),
                SUM(browsing_time),
                SUM(focus_time),
                SUM(workspace_switches),
                SUM(time_saved),
                SUM(bandwidth_saved)
             FROM daily_stats
             WHERE date >= ? AND date <= ?",
            [start_date, end_date],
            |row| {
                Ok(DailyStats {
                    date: format!("{} to {}", start_date, end_date),
                    trackers_blocked: row.get(0).unwrap_or(0),
                    ads_blocked: row.get(1).unwrap_or(0),
                    popups_blocked: row.get(2).unwrap_or(0),
                    pages_visited: row.get(3).unwrap_or(0),
                    tabs_opened: row.get(4).unwrap_or(0),
                    tabs_closed: row.get(5).unwrap_or(0),
                    browsing_time: row.get(6).unwrap_or(0),
                    focus_time: row.get(7).unwrap_or(0),
                    workspace_switches: row.get(8).unwrap_or(0),
                    time_saved: row.get(9).unwrap_or(0),
                    bandwidth_saved: row.get(10).unwrap_or(0),
                })
            },
        )
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(row)
}

/// Get event counts for a date range by event type
pub fn get_event_counts_by_type(
    conn: &Connection,
    start_date: &str,
    end_date: &str,
) -> HiWaveResult<Vec<(String, i64)>> {
    let start_ts = chrono::NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0);

    let end_ts = chrono::NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(23, 59, 59))
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(i64::MAX);

    let mut stmt = conn
        .prepare(
            "SELECT event_type, COUNT(*) as count
             FROM analytics_events
             WHERE created_at >= ? AND created_at <= ?
             GROUP BY event_type
             ORDER BY count DESC",
        )
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    let counts = stmt
        .query_map([start_ts, end_ts], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| HiWaveError::analytics(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(counts)
}

/// Update domain stats (increment visit count, update last visit)
pub fn update_domain_stats(conn: &Connection, domain: &str) -> HiWaveResult<()> {
    let now = Utc::now().timestamp();

    // Insert or update
    conn.execute(
        "INSERT INTO domain_stats (domain, visit_count, total_time, trackers_blocked, last_visit)
         VALUES (?, 1, 0, 0, ?)
         ON CONFLICT(domain) DO UPDATE SET
            visit_count = visit_count + 1,
            last_visit = ?",
        params![domain, now, now],
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(())
}

/// Increment tracker count for a domain
pub fn increment_domain_trackers(conn: &Connection, domain: &str) -> HiWaveResult<()> {
    conn.execute(
        "INSERT INTO domain_stats (domain, visit_count, total_time, trackers_blocked, last_visit)
         VALUES (?, 0, 0, 1, ?)
         ON CONFLICT(domain) DO UPDATE SET
            trackers_blocked = trackers_blocked + 1",
        params![domain, Utc::now().timestamp()],
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))?;

    Ok(())
}

/// Update workspace stats (increment counts)
pub fn update_workspace_stats(conn: &Connection, workspace_id: &str, event_type: &str) -> HiWaveResult<()> {
    match event_type {
        "tab_opened" => {
            conn.execute(
                "INSERT INTO workspace_stats (workspace_id, total_time, tab_count, visit_count)
                 VALUES (?, 0, 1, 0)
                 ON CONFLICT(workspace_id) DO UPDATE SET
                    tab_count = tab_count + 1",
                [workspace_id],
            )
            .map_err(|e| HiWaveError::analytics(e.to_string()))?;
        }
        "page_visit" => {
            conn.execute(
                "INSERT INTO workspace_stats (workspace_id, total_time, tab_count, visit_count)
                 VALUES (?, 0, 0, 1)
                 ON CONFLICT(workspace_id) DO UPDATE SET
                    visit_count = visit_count + 1",
                [workspace_id],
            )
            .map_err(|e| HiWaveError::analytics(e.to_string()))?;
        }
        _ => {}
    }

    Ok(())
}

/// Archive current analytics data to historical tables
pub fn archive_analytics_data(conn: &Connection) -> HiWaveResult<()> {
    let archive_timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();

    // Create archive tables if they don't exist
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS analytics_events_archive (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_type TEXT NOT NULL,
            domain TEXT,
            details TEXT,
            workspace_id TEXT,
            created_at INTEGER NOT NULL,
            archived_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS daily_stats_archive (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            date TEXT NOT NULL,
            trackers_blocked INTEGER DEFAULT 0,
            ads_blocked INTEGER DEFAULT 0,
            popups_blocked INTEGER DEFAULT 0,
            pages_visited INTEGER DEFAULT 0,
            tabs_opened INTEGER DEFAULT 0,
            tabs_closed INTEGER DEFAULT 0,
            browsing_time INTEGER DEFAULT 0,
            focus_time INTEGER DEFAULT 0,
            workspace_switches INTEGER DEFAULT 0,
            time_saved INTEGER DEFAULT 0,
            bandwidth_saved INTEGER DEFAULT 0,
            archived_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS domain_stats_archive (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            domain TEXT NOT NULL,
            visit_count INTEGER DEFAULT 0,
            total_time INTEGER DEFAULT 0,
            trackers_blocked INTEGER DEFAULT 0,
            last_visit INTEGER NOT NULL,
            archived_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS workspace_stats_archive (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            workspace_id TEXT NOT NULL,
            total_time INTEGER DEFAULT 0,
            tab_count INTEGER DEFAULT 0,
            visit_count INTEGER DEFAULT 0,
            archived_at TEXT NOT NULL
        );
        "#,
    )
    .map_err(|e| HiWaveError::analytics(format!("Failed to create archive tables: {}", e)))?;

    // Copy data to archive tables with timestamp
    conn.execute(
        "INSERT INTO analytics_events_archive (event_type, domain, details, workspace_id, created_at, archived_at)
         SELECT event_type, domain, details, workspace_id, created_at, ?
         FROM analytics_events",
        [&archive_timestamp],
    )
    .map_err(|e| HiWaveError::analytics(format!("Failed to archive events: {}", e)))?;

    conn.execute(
        "INSERT INTO daily_stats_archive (date, trackers_blocked, ads_blocked, popups_blocked,
            pages_visited, tabs_opened, tabs_closed, browsing_time, focus_time, workspace_switches,
            time_saved, bandwidth_saved, archived_at)
         SELECT date, trackers_blocked, ads_blocked, popups_blocked,
            pages_visited, tabs_opened, tabs_closed, browsing_time, focus_time, workspace_switches,
            time_saved, bandwidth_saved, ?
         FROM daily_stats",
        [&archive_timestamp],
    )
    .map_err(|e| HiWaveError::analytics(format!("Failed to archive daily stats: {}", e)))?;

    conn.execute(
        "INSERT INTO domain_stats_archive (domain, visit_count, total_time, trackers_blocked, last_visit, archived_at)
         SELECT domain, visit_count, total_time, trackers_blocked, last_visit, ?
         FROM domain_stats",
        [&archive_timestamp],
    )
    .map_err(|e| HiWaveError::analytics(format!("Failed to archive domain stats: {}", e)))?;

    conn.execute(
        "INSERT INTO workspace_stats_archive (workspace_id, total_time, tab_count, visit_count, archived_at)
         SELECT workspace_id, total_time, tab_count, visit_count, ?
         FROM workspace_stats",
        [&archive_timestamp],
    )
    .map_err(|e| HiWaveError::analytics(format!("Failed to archive workspace stats: {}", e)))?;

    // Clear current data
    conn.execute("DELETE FROM analytics_events", [])
        .map_err(|e| HiWaveError::analytics(format!("Failed to clear events: {}", e)))?;
    conn.execute("DELETE FROM daily_stats", [])
        .map_err(|e| HiWaveError::analytics(format!("Failed to clear daily stats: {}", e)))?;
    conn.execute("DELETE FROM domain_stats", [])
        .map_err(|e| HiWaveError::analytics(format!("Failed to clear domain stats: {}", e)))?;
    conn.execute("DELETE FROM workspace_stats", [])
        .map_err(|e| HiWaveError::analytics(format!("Failed to clear workspace stats: {}", e)))?;

    Ok(())
}

/// Estimates for resource savings from blocking
const TIME_SAVED_PER_BLOCK_SECS: i64 = 2; // ~2 seconds saved per tracker/ad blocked
const BANDWIDTH_SAVED_PER_BLOCK_BYTES: i64 = 51200; // ~50 KB saved per tracker/ad blocked

/// Update time and bandwidth saved for blocking events
pub fn update_savings(conn: &Connection, date: &str) -> HiWaveResult<()> {
    // Add time saved (2 seconds) and bandwidth saved (50 KB)
    conn.execute(
        "UPDATE daily_stats SET time_saved = time_saved + ?, bandwidth_saved = bandwidth_saved + ? WHERE date = ?",
        params![TIME_SAVED_PER_BLOCK_SECS, BANDWIDTH_SAVED_PER_BLOCK_BYTES, date],
    )
    .map_err(|e| HiWaveError::analytics(e.to_string()))?;
    Ok(())
}
