pub mod database;
pub mod types;

use chrono::Utc;
use hiwave_core::error::{HiWaveError, HiWaveResult};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use types::*;

/// Main analytics service
pub struct Analytics {
    conn: Arc<Mutex<Connection>>,
    settings: Arc<Mutex<ReportSettings>>,
}

impl Analytics {
    /// Create new analytics instance with database at given path
    pub fn new(db_path: PathBuf) -> HiWaveResult<Self> {
        let conn = database::init_database(&db_path)?;
        let settings = database::get_settings(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            settings: Arc::new(Mutex::new(settings)),
        })
    }

    /// Check if analytics tracking is enabled
    pub fn is_enabled(&self) -> bool {
        self.settings
            .lock()
            .unwrap()
            .enabled
    }

    /// Track an analytics event
    pub fn track(&self, event: AnalyticsEvent, workspace_id: Option<&str>) -> HiWaveResult<()> {
        if !self.is_enabled() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Insert event
        database::insert_event(&conn, &event, workspace_id)?;

        // Update daily stats
        database::get_or_create_today_stats(&conn)?;

        match &event {
            AnalyticsEvent::TrackerBlocked { .. } => {
                database::increment_daily_stat(&conn, &today, "trackers_blocked")?;
                database::update_savings(&conn, &today)?;
            }
            AnalyticsEvent::AdBlocked { .. } => {
                database::increment_daily_stat(&conn, &today, "ads_blocked")?;
                database::update_savings(&conn, &today)?;
            }
            AnalyticsEvent::PopupBlocked { .. } => {
                database::increment_daily_stat(&conn, &today, "popups_blocked")?;
                database::update_savings(&conn, &today)?;
            }
            AnalyticsEvent::PageVisit { .. } => {
                database::increment_daily_stat(&conn, &today, "pages_visited")?;
            }
            AnalyticsEvent::TabOpened => {
                database::increment_daily_stat(&conn, &today, "tabs_opened")?;
            }
            AnalyticsEvent::TabClosed { .. } => {
                database::increment_daily_stat(&conn, &today, "tabs_closed")?;
            }
            AnalyticsEvent::WorkspaceSwitch { .. } => {
                database::increment_daily_stat(&conn, &today, "workspace_switches")?;
            }
            AnalyticsEvent::FocusEnd { duration_secs, .. } => {
                database::add_daily_time(&conn, &today, "focus_time", *duration_secs)?;
            }
            AnalyticsEvent::SessionEnd { duration_secs } => {
                database::add_daily_time(&conn, &today, "browsing_time", *duration_secs)?;
            }
            _ => {}
        }

        Ok(())
    }

    /// Track a tracker being blocked
    pub fn track_tracker_blocked(&self, domain: &str, workspace_id: Option<&str>) -> HiWaveResult<()> {
        let conn = self.conn.lock().unwrap();
        database::increment_domain_trackers(&conn, domain)?;
        drop(conn);

        self.track(
            AnalyticsEvent::TrackerBlocked {
                domain: domain.to_string(),
            },
            workspace_id,
        )
    }

    /// Track an ad being blocked
    pub fn track_ad_blocked(&self, domain: &str, workspace_id: Option<&str>) -> HiWaveResult<()> {
        self.track(
            AnalyticsEvent::AdBlocked {
                domain: domain.to_string(),
            },
            workspace_id,
        )
    }

    /// Track a popup being blocked
    pub fn track_popup_blocked(&self, domain: &str, workspace_id: Option<&str>) -> HiWaveResult<()> {
        self.track(
            AnalyticsEvent::PopupBlocked {
                domain: domain.to_string(),
            },
            workspace_id,
        )
    }

    /// Track a page visit
    pub fn track_page_visit(&self, domain: &str, workspace_id: Option<&str>) -> HiWaveResult<()> {
        let conn = self.conn.lock().unwrap();
        database::update_domain_stats(&conn, domain)?;
        if let Some(ws_id) = workspace_id {
            database::update_workspace_stats(&conn, ws_id, "page_visit")?;
        }
        drop(conn);

        self.track(
            AnalyticsEvent::PageVisit {
                domain: domain.to_string(),
            },
            workspace_id,
        )
    }

    /// Track a tab being opened
    pub fn track_tab_opened(&self, workspace_id: Option<&str>) -> HiWaveResult<()> {
        if let Some(ws_id) = workspace_id {
            let conn = self.conn.lock().unwrap();
            database::update_workspace_stats(&conn, ws_id, "tab_opened")?;
        }

        self.track(AnalyticsEvent::TabOpened, workspace_id)
    }

    /// Track a tab being closed
    pub fn track_tab_closed(&self, duration_secs: i64, workspace_id: Option<&str>) -> HiWaveResult<()> {
        self.track(AnalyticsEvent::TabClosed { duration_secs }, workspace_id)
    }

    /// Track tab moved to shelf
    pub fn track_tab_to_shelf(&self, domain: &str, workspace_id: Option<&str>) -> HiWaveResult<()> {
        self.track(
            AnalyticsEvent::TabToShelf {
                domain: domain.to_string(),
            },
            workspace_id,
        )
    }

    /// Track tab restored from shelf
    pub fn track_tab_from_shelf(&self, domain: &str, workspace_id: Option<&str>) -> HiWaveResult<()> {
        self.track(
            AnalyticsEvent::TabFromShelf {
                domain: domain.to_string(),
            },
            workspace_id,
        )
    }

    /// Track workspace switch
    pub fn track_workspace_switch(&self, from: &str, to: &str) -> HiWaveResult<()> {
        self.track(
            AnalyticsEvent::WorkspaceSwitch {
                from: from.to_string(),
                to: to.to_string(),
            },
            Some(to),
        )
    }

    /// Track focus mode start
    pub fn track_focus_start(&self, domain: &str, workspace_id: Option<&str>) -> HiWaveResult<()> {
        self.track(
            AnalyticsEvent::FocusStart {
                domain: domain.to_string(),
            },
            workspace_id,
        )
    }

    /// Track focus mode end
    pub fn track_focus_end(&self, domain: &str, duration_secs: i64, workspace_id: Option<&str>) -> HiWaveResult<()> {
        self.track(
            AnalyticsEvent::FocusEnd {
                domain: domain.to_string(),
                duration_secs,
            },
            workspace_id,
        )
    }

    /// Track session start
    pub fn track_session_start(&self) -> HiWaveResult<()> {
        self.track(AnalyticsEvent::SessionStart, None)
    }

    /// Track session end
    pub fn track_session_end(&self, duration_secs: i64) -> HiWaveResult<()> {
        self.track(AnalyticsEvent::SessionEnd { duration_secs }, None)
    }

    /// Get daily stats for a specific date
    pub fn get_daily_stats(&self, date: &str) -> HiWaveResult<DailyStats> {
        let conn = self.conn.lock().unwrap();
        database::get_daily_stats(&conn, date)
    }

    /// Get today's stats
    pub fn get_today_stats(&self) -> HiWaveResult<DailyStats> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        self.get_daily_stats(&today)
    }

    /// Get top domains by visit count
    pub fn get_top_domains(&self, limit: usize) -> HiWaveResult<Vec<DomainStats>> {
        let conn = self.conn.lock().unwrap();
        database::get_top_domains(&conn, limit)
    }

    /// Get workspace statistics
    pub fn get_workspace_stats(&self) -> HiWaveResult<Vec<WorkspaceStats>> {
        let conn = self.conn.lock().unwrap();
        database::get_workspace_stats(&conn)
    }

    /// Get report settings
    pub fn get_settings(&self) -> ReportSettings {
        self.settings.lock().unwrap().clone()
    }

    /// Update report settings
    pub fn update_settings(&self, settings: ReportSettings) -> HiWaveResult<()> {
        let conn = self.conn.lock().unwrap();
        database::update_settings(&conn, &settings)?;
        *self.settings.lock().unwrap() = settings;
        Ok(())
    }

    /// Clean up old events based on retention settings
    pub fn cleanup_old_data(&self) -> HiWaveResult<usize> {
        let retention_days = self.settings.lock().unwrap().retention_days;
        let conn = self.conn.lock().unwrap();
        database::cleanup_old_events(&conn, retention_days)
    }

    /// Clear all analytics data (for privacy)
    pub fn clear_all_data(&self) -> HiWaveResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM analytics_events", [])
            .map_err(|e| HiWaveError::analytics(e.to_string()))?;
        conn.execute("DELETE FROM daily_stats", [])
            .map_err(|e| HiWaveError::analytics(e.to_string()))?;
        conn.execute("DELETE FROM domain_stats", [])
            .map_err(|e| HiWaveError::analytics(e.to_string()))?;
        conn.execute("DELETE FROM workspace_stats", [])
            .map_err(|e| HiWaveError::analytics(e.to_string()))?;
        Ok(())
    }

    /// Archive current analytics data to historical tables
    /// This preserves the data in archive tables with a timestamp
    pub fn archive_data(&self) -> HiWaveResult<()> {
        let conn = self.conn.lock().unwrap();
        database::archive_analytics_data(&conn)
    }

    /// Generate a report for a custom date range
    pub fn generate_report(&self, start_date: &str, end_date: &str) -> HiWaveResult<Report> {
        let conn = self.conn.lock().unwrap();

        // Get aggregated totals
        let total_stats = database::aggregate_stats_range(&conn, start_date, end_date)?;

        // Get daily breakdown
        let daily_breakdown = database::get_daily_stats_range(&conn, start_date, end_date)?;

        // Get top domains
        let top_domains = database::get_top_domains(&conn, 10)?;

        // Get workspace breakdown
        let workspace_breakdown = database::get_workspace_stats(&conn)?;

        Ok(Report {
            start_date: start_date.to_string(),
            end_date: end_date.to_string(),
            total_stats,
            daily_breakdown,
            top_domains,
            workspace_breakdown,
        })
    }

    /// Generate a weekly report (last 7 days)
    pub fn generate_weekly_report(&self) -> HiWaveResult<Report> {
        let end_date = Utc::now().format("%Y-%m-%d").to_string();
        let start_date = (Utc::now() - chrono::Duration::days(6))
            .format("%Y-%m-%d")
            .to_string();

        self.generate_report(&start_date, &end_date)
    }

    /// Generate a monthly report (last 30 days)
    pub fn generate_monthly_report(&self) -> HiWaveResult<Report> {
        let end_date = Utc::now().format("%Y-%m-%d").to_string();
        let start_date = (Utc::now() - chrono::Duration::days(29))
            .format("%Y-%m-%d")
            .to_string();

        self.generate_report(&start_date, &end_date)
    }

    /// Get stats for the last N days
    pub fn get_last_n_days_stats(&self, days: i64) -> HiWaveResult<Vec<DailyStats>> {
        let conn = self.conn.lock().unwrap();
        let end_date = Utc::now().format("%Y-%m-%d").to_string();
        let start_date = (Utc::now() - chrono::Duration::days(days - 1))
            .format("%Y-%m-%d")
            .to_string();

        database::get_daily_stats_range(&conn, &start_date, &end_date)
    }

    /// Get aggregated stats for the last N days
    pub fn get_last_n_days_total(&self, days: i64) -> HiWaveResult<DailyStats> {
        let conn = self.conn.lock().unwrap();
        let end_date = Utc::now().format("%Y-%m-%d").to_string();
        let start_date = (Utc::now() - chrono::Duration::days(days - 1))
            .format("%Y-%m-%d")
            .to_string();

        database::aggregate_stats_range(&conn, &start_date, &end_date)
    }

    /// Get event type breakdown for a date range
    pub fn get_event_breakdown(&self, start_date: &str, end_date: &str) -> HiWaveResult<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        database::get_event_counts_by_type(&conn, start_date, end_date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_analytics_initialization() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("analytics.db");
        let analytics = Analytics::new(db_path).unwrap();
        assert!(analytics.is_enabled());
    }

    #[test]
    fn test_track_event() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("analytics.db");
        let analytics = Analytics::new(db_path).unwrap();

        analytics.track_tracker_blocked("example.com", Some("default")).unwrap();

        let stats = analytics.get_today_stats().unwrap();
        assert_eq!(stats.trackers_blocked, 1);
    }

    #[test]
    fn test_settings_update() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("analytics.db");
        let analytics = Analytics::new(db_path).unwrap();

        let mut settings = analytics.get_settings();
        settings.retention_days = 30;
        analytics.update_settings(settings).unwrap();

        let updated = analytics.get_settings();
        assert_eq!(updated.retention_days, 30);
    }

    #[test]
    fn test_report_generation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("analytics.db");
        let analytics = Analytics::new(db_path).unwrap();

        // Track some events
        analytics.track_tracker_blocked("example.com", Some("1")).unwrap();
        analytics.track_page_visit("example.com", Some("1")).unwrap();
        analytics.track_tab_opened(Some("1")).unwrap();

        // Generate a weekly report
        let report = analytics.generate_weekly_report().unwrap();

        // Verify report structure
        assert!(!report.start_date.is_empty());
        assert!(!report.end_date.is_empty());
        assert!(report.total_stats.trackers_blocked >= 1);
        assert!(report.total_stats.pages_visited >= 1);
        assert!(report.total_stats.tabs_opened >= 1);
    }

    #[test]
    fn test_domain_and_workspace_stats() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("analytics.db");
        let analytics = Analytics::new(db_path).unwrap();

        // Track events with domain and workspace
        analytics.track_page_visit("github.com", Some("workspace-1")).unwrap();
        analytics.track_page_visit("github.com", Some("workspace-1")).unwrap();
        analytics.track_tracker_blocked("github.com", Some("workspace-1")).unwrap();
        analytics.track_tab_opened(Some("workspace-1")).unwrap();

        // Get domain stats
        let domains = analytics.get_top_domains(10).unwrap();
        assert!(!domains.is_empty());
        let github = domains.iter().find(|d| d.domain == "github.com");
        assert!(github.is_some());
        let github = github.unwrap();
        assert_eq!(github.visit_count, 2);
        assert_eq!(github.trackers_blocked, 1);

        // Get workspace stats
        let workspaces = analytics.get_workspace_stats().unwrap();
        assert!(!workspaces.is_empty());
        let ws1 = workspaces.iter().find(|w| w.workspace_id == "workspace-1");
        assert!(ws1.is_some());
        let ws1 = ws1.unwrap();
        assert_eq!(ws1.visit_count, 2);
        assert_eq!(ws1.tab_count, 1);
    }

    #[test]
    fn test_date_range_stats() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("analytics.db");
        let analytics = Analytics::new(db_path).unwrap();

        // Track some events
        analytics.track_page_visit("example.com", Some("1")).unwrap();
        analytics.track_tracker_blocked("tracker.com", Some("1")).unwrap();

        // Get last 7 days stats
        let stats = analytics.get_last_n_days_total(7).unwrap();
        assert!(stats.pages_visited >= 1);
        assert!(stats.trackers_blocked >= 1);

        // Get daily breakdown
        let daily = analytics.get_last_n_days_stats(7).unwrap();
        assert!(!daily.is_empty());
    }
}
