use std::{
    collections::HashSet,
    fs,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use chrono::Utc;
use eframe::{
    App, CreationContext, Frame,
    egui::{self, Context, FontData, FontDefinitions, FontFamily, Layout, RichText},
};
use egui_extras::{Column, TableBuilder};

use crate::{
    domain::{GitHubAccount, InboxSnapshot, NotificationItem},
    github::{self, FetchError},
    storage::AccountStore,
};

pub const APP_NAME: &str = "Reminder";

pub const CJK_FONT_NAME: &str = "CJK_Fallback_Font";
const ACCOUNTS_PANEL_MIN_WIDTH: f32 = 140.0;
const ACCOUNTS_PANEL_MAX_WIDTH: f32 = 240.0;
const ACCOUNTS_PANEL_WIDTH_RATIO: f32 = 0.24;
const COMPACT_ACCOUNT_ROW_WIDTH: f32 = 180.0;
const STACKED_ACCOUNT_HEADER_WIDTH: f32 = 620.0;
const COMPACT_NOTIFICATION_WIDTH: f32 = 640.0;

#[cfg(target_os = "macos")]
const SYSTEM_FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Supplemental/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Supplemental/NotoSansCJK-Regular.ttc",
];

#[cfg(target_os = "windows")]
const SYSTEM_FONT_CANDIDATES: &[&str] = &[
    "C:\\Windows\\Fonts\\malgun.ttf",
    "C:\\Windows\\Fonts\\malgunbd.ttf",
    "C:\\Windows\\Fonts\\YuGothM.ttc",
];

#[cfg(target_os = "linux")]
const SYSTEM_FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansKR-Regular.otf",
];

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
const SYSTEM_FONT_CANDIDATES: &[&str] = &[];
const AUTO_REFRESH_INTERVAL_SECS: u64 = 180;

pub struct ReminderApp {
    account_form: AccountForm,
    accounts: Vec<AccountState>,
    selected_account_login: Option<String>,
    secret_store: Option<AccountStore>,
    storage_warning: Option<String>,
    global_error: Option<String>,
    auto_refresh: BatchRefreshScheduler,
}

impl ReminderApp {
    pub fn new(cc: &CreationContext<'_>) -> Self {
        install_international_fonts(&cc.egui_ctx);

        let mut app = Self {
            account_form: AccountForm::default(),
            accounts: Vec::new(),
            selected_account_login: None,
            secret_store: None,
            storage_warning: None,
            global_error: None,
            auto_refresh: BatchRefreshScheduler::new(Duration::from_secs(
                AUTO_REFRESH_INTERVAL_SECS,
            )),
        };

        match AccountStore::initialize() {
            Ok(store) => {
                match store.hydrate() {
                    Ok(outcome) => {
                        for profile in outcome.profiles {
                            let mut state = AccountState::new(profile);
                            state.start_refresh();
                            app.accounts.push(state);
                        }
                    }
                    Err(err) => {
                        app.storage_warning =
                            Some(format!("Failed to restore saved accounts: {err}"))
                    }
                }
                app.secret_store = Some(store);
            }
            Err(err) => {
                app.storage_warning = Some(format!(
                    "Local token storage is unavailable; tokens cannot be persisted ({err})."
                ));
            }
        }

        app.ensure_selected_account();
        app.auto_refresh.mark_triggered();

        app
    }

    fn add_account(&mut self) {
        if self.account_form.login.trim().is_empty() || self.account_form.token.trim().is_empty() {
            self.account_form.form_error =
                Some("Both the login and a Personal Access Token are required.".to_owned());
            return;
        }

        if self.accounts.iter().any(|account| {
            account
                .profile
                .login
                .eq_ignore_ascii_case(self.account_form.login.trim())
        }) {
            self.account_form.form_error =
                Some("This GitHub login is already being tracked.".to_owned());
            return;
        }

        let profile = GitHubAccount {
            login: self.account_form.login.trim().to_owned(),
            token: self.account_form.token.trim().to_owned(),
        };
        let selected_login = profile.login.clone();

        if let Some(store) = &self.secret_store {
            if let Err(err) = store.persist_profile(&profile) {
                self.account_form.form_error =
                    Some(format!("Unable to persist credentials locally: {err}"));
                return;
            }
        } else {
            self.account_form.form_error = Some(
                "Local token storage is not available; cannot add new accounts right now."
                    .to_owned(),
            );
            return;
        }

        let mut state = AccountState::new(profile);
        state.start_refresh();
        self.auto_refresh.mark_triggered();
        self.accounts.push(state);
        self.selected_account_login = Some(selected_login);
        self.account_form = AccountForm::default();
    }

    fn remove_account_at(&mut self, idx: usize) {
        if idx >= self.accounts.len() {
            return;
        }

        let login = self.accounts[idx].profile.login.clone();
        if let Some(store) = &self.secret_store
            && let Err(err) = store.forget(&login)
        {
            self.global_error = Some(format!("Failed to remove credentials for {login}: {err}"));
        }

        self.accounts.remove(idx);
        self.ensure_selected_account();
    }

    fn ensure_selected_account(&mut self) {
        let Some(selected_login) = self.selected_account_login.as_deref() else {
            self.selected_account_login = self
                .accounts
                .first()
                .map(|account| account.profile.login.clone());
            return;
        };

        if self
            .accounts
            .iter()
            .any(|account| account.profile.login == selected_login)
        {
            return;
        }

        self.selected_account_login = self
            .accounts
            .first()
            .map(|account| account.profile.login.clone());
    }

    fn poll_jobs(&mut self) {
        for account in &mut self.accounts {
            account.poll_job();
            account.poll_action_jobs();
        }
    }

    fn maybe_auto_refresh(&mut self) {
        if !self.auto_refresh.should_trigger() {
            return;
        }

        let mut triggered = false;
        let stale_after = Duration::from_secs(AUTO_REFRESH_INTERVAL_SECS);
        for account in &mut self.accounts {
            if account.pending_job.is_some() {
                continue;
            }
            if account.needs_refresh(stale_after) {
                account.start_refresh();
                triggered = true;
            }
        }

        if triggered {
            self.auto_refresh.mark_triggered();
        }
    }

    fn render_side_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Accounts");
        ui.separator();

        if let Some(warning) = &self.storage_warning {
            ui.colored_label(ui.visuals().warn_fg_color, warning);
            ui.separator();
        }

        ui.label("GitHub username");
        ui.text_edit_singleline(&mut self.account_form.login);

        ui.label("Personal access token");
        ui.add(
            egui::TextEdit::singleline(&mut self.account_form.token)
                .password(true)
                .hint_text("ghp_..."),
        );

        let add_enabled = !self.account_form.login.trim().is_empty()
            && !self.account_form.token.trim().is_empty();
        if ui
            .add_enabled(add_enabled, egui::Button::new("Add account"))
            .clicked()
        {
            self.add_account();
        }

        if let Some(error) = &self.account_form.form_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }

        ui.separator();
        ui.label("Tracked accounts");
        if self.accounts.is_empty() {
            ui.weak("No accounts yet.");
        } else {
            let compact_rows = uses_compact_account_rows(ui.available_width());
            let mut selected_login = None;
            let mut refresh_idx = None;
            let mut remove_idx = None;
            for (idx, account) in self.accounts.iter_mut().enumerate() {
                let overview = account_overview(account);
                let pending = account.pending_job.is_some();
                let has_error = account.last_error.is_some();
                let is_selected =
                    self.selected_account_login.as_deref() == Some(account.profile.login.as_str());
                let heading = tracked_account_heading(ui, account, is_selected, overview);

                ui.group(|group| {
                    if group.selectable_label(is_selected, heading).clicked() {
                        selected_login = Some(account.profile.login.clone());
                    }

                    if compact_rows {
                        render_tracked_account_badges(group, overview, pending, has_error);
                        group.horizontal_wrapped(|row| {
                            if row.small_button("Refresh").clicked() {
                                refresh_idx = Some(idx);
                            }
                            if row.small_button("Remove").clicked() {
                                remove_idx = Some(idx);
                            }
                        });
                    } else {
                        group.horizontal_wrapped(|row| {
                            render_tracked_account_badges(row, overview, pending, has_error);
                            if row.small_button("Refresh").clicked() {
                                refresh_idx = Some(idx);
                            }
                            if row.small_button("Remove").clicked() {
                                remove_idx = Some(idx);
                            }
                        });
                    }
                });
            }
            if let Some(login) = selected_login {
                self.selected_account_login = Some(login);
            }
            if let Some(idx) = refresh_idx
                && let Some(account) = self.accounts.get_mut(idx)
            {
                account.start_refresh();
                self.auto_refresh.mark_triggered();
            }
            if let Some(idx) = remove_idx {
                self.remove_account_at(idx);
            }
        }
    }

    fn render_dashboard(&mut self, ui: &mut egui::Ui) {
        self.render_global_error(ui);

        if self.accounts.is_empty() {
            ui.centered_and_justified(|center| {
                center.label("Add at least one GitHub account to start aggregating notifications.");
            });
            return;
        }

        let Some(selected_login) = self.selected_account_login.as_deref() else {
            ui.centered_and_justified(|center| {
                center.label("Select an account on the left to view notifications.");
            });
            return;
        };

        let Some(selected_idx) = self
            .accounts
            .iter()
            .position(|account| account.profile.login == selected_login)
        else {
            ui.centered_and_justified(|center| {
                center.label("Select an account on the left to view notifications.");
            });
            return;
        };

        egui::ScrollArea::vertical().show(ui, |area| {
            let account = &mut self.accounts[selected_idx];
            account.clear_new_notifications();
            let account_id = account.profile.login.clone();
            area.push_id(account_id, |ui| {
                render_account_card(ui, account);
            });
        });
    }

    fn render_global_error(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.global_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            ui.add_space(8.0);
        }
    }
}

fn render_account_card(ui: &mut egui::Ui, account: &mut AccountState) {
    ui.group(|group| {
        render_account_header(group, account);
        render_account_status(group, account);
        render_account_body(group, account);
    });
    ui.add_space(12.0);
}

fn render_account_header(group: &mut egui::Ui, account: &mut AccountState) {
    if uses_stacked_account_header(group.available_width()) {
        group.vertical(|column| {
            column.horizontal_wrapped(|row| {
                row.heading(format!("Account: {}", account.profile.login));
                if row
                    .small_button(if account.expanded {
                        "Hide notifications"
                    } else {
                        "Show notifications"
                    })
                    .clicked()
                {
                    account.expanded = !account.expanded;
                }
            });
            render_view_mode_toggle(column, account);
            let search_width = column.available_width();
            column.add(
                egui::TextEdit::singleline(&mut account.search_query)
                    .hint_text("Search…")
                    .desired_width(search_width),
            );
        });
    } else {
        group.horizontal(|row| {
            row.heading(format!("Account: {}", account.profile.login));
            if row
                .small_button(if account.expanded {
                    "Hide notifications"
                } else {
                    "Show notifications"
                })
                .clicked()
            {
                account.expanded = !account.expanded;
            }
            row.with_layout(Layout::right_to_left(egui::Align::Center), |lane| {
                lane.add(
                    egui::TextEdit::singleline(&mut account.search_query)
                        .hint_text("Search…")
                        .desired_width(160.0),
                );
                lane.add_space(8.0);
                render_view_mode_toggle(lane, account);
            });
        });
    }
}

fn render_view_mode_toggle(ui: &mut egui::Ui, account: &mut AccountState) {
    ui.selectable_value(&mut account.view_mode, AccountViewMode::Grouped, "Grouped")
        .on_hover_text("Group notifications into review requests, mentions, and everything else.");
    ui.selectable_value(
        &mut account.view_mode,
        AccountViewMode::Inbox,
        "Unified inbox",
    )
    .on_hover_text("Show every GitHub notification in one list, like GitHub's inbox.");
}

fn render_account_status(group: &mut egui::Ui, account: &AccountState) {
    if let Some(inbox) = &account.inbox {
        group.label(format!(
            "Last synced {} UTC",
            inbox.fetched_at.format("%Y-%m-%d %H:%M:%S")
        ));
    } else {
        group.label("No data fetched yet.");
    }

    if let Some(err) = &account.last_error {
        group.colored_label(group.visuals().error_fg_color, err);
    } else if account.pending_job.is_some() {
        group.label("Fetching latest notifications...");
    }
}

fn render_account_body(group: &mut egui::Ui, account: &mut AccountState) {
    if !account.expanded {
        if account.inbox.is_none() {
            group.separator();
            group.weak("No data loaded yet.");
        }
        return;
    }

    if account.inbox.is_some() {
        group.separator();
        let filter = SearchFilter::new(&account.search_query);
        let actions = match account.view_mode {
            AccountViewMode::Inbox => render_unified_inbox_section(group, account, &filter),
            AccountViewMode::Grouped => render_bucket_sections(group, account, &filter),
        };
        for action in actions {
            match action {
                AccountAction::Done(id) => account.request_mark_done(id),
                AccountAction::Seen(id) => account.mark_notification_seen(&id),
                AccountAction::Read(id) => account.request_mark_read(id),
            }
        }
    }
}

fn render_unified_inbox_section(
    group: &mut egui::Ui,
    account: &mut AccountState,
    filter: &SearchFilter,
) -> Vec<AccountAction> {
    let inflight_done = account.inflight_done.clone();
    let inbox = account.inbox.as_ref().expect("checked by caller");
    let notifications: Vec<_> = inbox.notifications.iter().collect();

    let (actions, cleared_highlight) = render_notification_section(
        group,
        "Inbox",
        notifications,
        "You're all caught up 🎉",
        filter,
        &inflight_done,
        account.highlights.contains(&SectionKind::Inbox),
    );
    if cleared_highlight {
        account.highlights.remove(&SectionKind::Inbox);
    }
    actions
}

fn render_bucket_sections(
    group: &mut egui::Ui,
    account: &mut AccountState,
    filter: &SearchFilter,
) -> Vec<AccountAction> {
    const REVIEW_REQUEST_REASON: &str = "review_requested";
    const MENTION_REASONS: &[&str] = &["mention", "team_mention"];

    // Show both seen and unseen items in their contextual buckets; the Done section
    // is temporarily disabled to avoid splitting the feed.
    let mut actions = Vec::new();
    let inflight_done = account.inflight_done.clone();
    let inbox = account.inbox.as_ref().expect("checked by caller");

    let review_requests: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| item.reason == REVIEW_REQUEST_REASON)
        .collect();

    let (section_actions, cleared_highlight) = render_notification_section(
        group,
        "Review requests",
        review_requests,
        "No pending review requests.",
        filter,
        &inflight_done,
        account.highlights.contains(&SectionKind::ReviewRequests),
    );
    actions.extend(section_actions);
    if cleared_highlight {
        account.highlights.remove(&SectionKind::ReviewRequests);
    }
    group.separator();

    let mentions: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| MENTION_REASONS.contains(&item.reason.as_str()))
        .collect();
    let (section_actions, cleared_highlight) = render_notification_section(
        group,
        "Mentions",
        mentions,
        "No recent mentions.",
        filter,
        &inflight_done,
        account.highlights.contains(&SectionKind::Mentions),
    );
    actions.extend(section_actions);
    if cleared_highlight {
        account.highlights.remove(&SectionKind::Mentions);
    }
    group.separator();

    let other: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| {
            item.reason != REVIEW_REQUEST_REASON && !MENTION_REASONS.contains(&item.reason.as_str())
        })
        .collect();
    let (section_actions, cleared_highlight) = render_notification_section(
        group,
        "Notifications",
        other,
        "You're all caught up 🎉",
        filter,
        &inflight_done,
        account.highlights.contains(&SectionKind::Notifications),
    );
    actions.extend(section_actions);
    if cleared_highlight {
        account.highlights.remove(&SectionKind::Notifications);
    }

    actions
}

// -----------------------------------------------------------------------------
// Font configuration
// -----------------------------------------------------------------------------

fn install_international_fonts(ctx: &Context) {
    // CJK reviewers reported tofu glyphs because egui's built-in Latin fonts
    // do not cover Hangul. Prefer system-provided CJK families to avoid bloating
    // the binary, but fall back to the bundled font when the optional
    let Some(font_data) = resolve_cjk_font_data() else {
        eprintln!("Warning: no CJK-capable font found; Some glyphs may fail to render.");
        return;
    };

    let mut definitions = FontDefinitions::default();
    definitions
        .font_data
        .insert(CJK_FONT_NAME.to_owned(), font_data.into());

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .insert(0, CJK_FONT_NAME.to_owned());
    }

    ctx.set_fonts(definitions);
}

fn resolve_cjk_font_data() -> Option<FontData> {
    load_system_cjk_font()
}

fn load_system_cjk_font() -> Option<FontData> {
    for candidate in SYSTEM_FONT_CANDIDATES {
        if let Ok(bytes) = fs::read(candidate) {
            return Some(FontData::from_owned(bytes));
        }
    }
    None
}

impl App for ReminderApp {
    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        self.poll_jobs();
        self.maybe_auto_refresh();
        self.ensure_selected_account();

        let accounts_panel_width = responsive_accounts_panel_width(ctx.available_rect().width());

        egui::SidePanel::left("accounts_panel")
            .exact_width(accounts_panel_width)
            .show(ctx, |ui| self.render_side_panel(ui));

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_dashboard(ui);
        });

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

// -----------------------------------------------------------------------------
// Account state & background jobs
// -----------------------------------------------------------------------------

struct AccountState {
    profile: GitHubAccount,
    inbox: Option<InboxSnapshot>,
    new_notification_ids: HashSet<String>,
    last_error: Option<String>,
    pending_job: Option<PendingJob>,
    pending_actions: Vec<NotificationActionJob>,
    expanded: bool,
    view_mode: AccountViewMode,
    search_query: String,
    inflight_done: HashSet<String>,
    highlights: HashSet<SectionKind>,
}

impl AccountState {
    fn new(profile: GitHubAccount) -> Self {
        Self {
            profile,
            inbox: None,
            new_notification_ids: HashSet::new(),
            last_error: None,
            pending_job: None,
            pending_actions: Vec::new(),
            expanded: true,
            view_mode: AccountViewMode::Inbox,
            search_query: String::new(),
            inflight_done: HashSet::new(),
            highlights: HashSet::new(),
        }
    }

    fn start_refresh(&mut self) {
        let profile = self.profile.clone();
        self.last_error = None;
        self.pending_job = Some(PendingJob::spawn(profile));
    }

    fn poll_job(&mut self) {
        if let Some(job) = &mut self.pending_job
            && let Some(result) = job.try_take()
        {
            self.pending_job = None;
            match result {
                Ok(inbox) => {
                    let new_notification_ids =
                        collect_new_notification_ids(self.inbox.as_ref(), &inbox);
                    let current_ids: HashSet<_> = inbox
                        .notifications
                        .iter()
                        .map(|item| item.thread_id.as_str())
                        .collect();
                    self.new_notification_ids
                        .retain(|thread_id| current_ids.contains(thread_id.as_str()));
                    self.new_notification_ids.extend(new_notification_ids);
                    let previous_stats = self.inbox.as_ref().map(section_stats);
                    let next_stats = section_stats(&inbox);
                    if let Some(old) = previous_stats {
                        if next_stats.inbox.bumped_since(&old.inbox) {
                            self.highlights.insert(SectionKind::Inbox);
                        }
                        if next_stats
                            .review_requests
                            .bumped_since(&old.review_requests)
                        {
                            self.highlights.insert(SectionKind::ReviewRequests);
                        }
                        if next_stats.mentions.bumped_since(&old.mentions) {
                            self.highlights.insert(SectionKind::Mentions);
                        }
                        if next_stats.notifications.bumped_since(&old.notifications) {
                            self.highlights.insert(SectionKind::Notifications);
                        }
                    }

                    self.inbox = Some(inbox);
                    self.last_error = None;
                }
                Err(err) => {
                    self.last_error = Some(err.to_string());
                }
            }
        }
    }

    fn poll_action_jobs(&mut self) {
        let mut finished = Vec::new();
        self.pending_actions.retain(|job| match job.try_take() {
            None => true,
            Some(result) => {
                finished.push(result);
                false
            }
        });

        for outcome in finished {
            match outcome {
                Ok(thread_id) => self.handle_action_success(&thread_id),
                Err((thread_id, err)) => {
                    self.last_error = Some(err);
                    if let Some(id) = thread_id {
                        self.inflight_done.remove(&id);
                    }
                }
            }
        }
    }

    fn handle_action_success(&mut self, thread_id: &str) {
        if let Some(inbox) = &mut self.inbox
            && let Some(item) = inbox
                .notifications
                .iter_mut()
                .find(|item| item.thread_id == thread_id)
        {
            item.unread = false;
            // Consider the thread freshly read at the current timestamp so the
            // "Updated" badge clears unless new events arrive.
            item.last_read_at = Some(Utc::now());
        }
        self.inflight_done.remove(thread_id);
    }

    /// Mark a thread as seen the moment the user opens it so the UI reflects
    /// the visit without waiting for the next GitHub sync cycle.
    fn mark_notification_seen(&mut self, thread_id: &str) {
        if let Some(inbox) = &mut self.inbox
            && let Some(item) = inbox
                .notifications
                .iter_mut()
                .find(|item| item.thread_id == thread_id)
        {
            item.unread = false;
            // Advance the local last_read_at to the newest update to clear the
            // "Updated" badge unless more activity arrives later.
            item.last_read_at = Some(item.updated_at);
        }
    }

    fn request_mark_read(&mut self, thread_id: String) {
        if self.inflight_done.contains(&thread_id) {
            return;
        }
        let profile = self.profile.clone();
        let job = NotificationActionJob::mark_read(profile, thread_id.clone());
        self.pending_actions.push(job);
        self.inflight_done.insert(thread_id);
    }

    fn request_mark_done(&mut self, thread_id: String) {
        if self.inflight_done.contains(&thread_id) {
            return;
        }
        let profile = self.profile.clone();
        let job = NotificationActionJob::mark_done(profile, thread_id.clone());
        self.pending_actions.push(job);
        self.inflight_done.insert(thread_id);
    }

    fn needs_refresh(&self, threshold: Duration) -> bool {
        match &self.inbox {
            None => true,
            Some(inbox) => match chrono::Duration::from_std(threshold) {
                Ok(delta) => (Utc::now() - inbox.fetched_at) >= delta,
                Err(_) => true,
            },
        }
    }

    fn clear_new_notifications(&mut self) {
        self.new_notification_ids.clear();
    }
}

struct PendingJob {
    receiver: Receiver<github::FetchOutcome>,
}

impl PendingJob {
    fn spawn(profile: GitHubAccount) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let outcome = (|| -> github::FetchOutcome {
                let client = github::build_client()?;
                github::fetch_inbox(&client, &profile)
            })();
            let _ = tx.send(outcome);
        });
        Self { receiver: rx }
    }

    fn try_take(&self) -> Option<github::FetchOutcome> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(FetchError::BackgroundWorkerGone)),
        }
    }
}

type NotificationActionResult = Result<String, (Option<String>, String)>;

struct NotificationActionJob {
    receiver: Receiver<NotificationActionResult>,
}

impl NotificationActionJob {
    fn mark_done(profile: GitHubAccount, thread_id: String) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let outcome = Self::mark_done_worker(profile, thread_id);
            let _ = tx.send(outcome);
        });
        Self { receiver: rx }
    }

    fn mark_done_worker(profile: GitHubAccount, thread_id: String) -> NotificationActionResult {
        let client =
            github::build_client().map_err(|err| (Some(thread_id.clone()), err.to_string()))?;
        github::mark_notification_done(&client, &profile, &thread_id)
            .map_err(|err| (Some(thread_id.clone()), err.to_string()))?;
        Ok(thread_id)
    }

    fn mark_read(profile: GitHubAccount, thread_id: String) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let outcome = Self::mark_read_worker(profile, thread_id);
            let _ = tx.send(outcome);
        });
        Self { receiver: rx }
    }

    fn mark_read_worker(profile: GitHubAccount, thread_id: String) -> NotificationActionResult {
        let client =
            github::build_client().map_err(|err| (Some(thread_id.clone()), err.to_string()))?;
        github::mark_notification_read(&client, &profile, &thread_id)
            .map_err(|err| (Some(thread_id.clone()), err.to_string()))?;
        Ok(thread_id)
    }

    fn try_take(&self) -> Option<NotificationActionResult> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err((
                None,
                "Notification action worker disconnected".to_owned(),
            ))),
        }
    }
}

// -----------------------------------------------------------------------------
// UI helpers
// -----------------------------------------------------------------------------

fn render_notification_section(
    group: &mut egui::Ui,
    title: &str,
    subset: Vec<&NotificationItem>,
    empty_label: &'static str,
    filter: &SearchFilter,
    inflight_done: &HashSet<String>,
    highlight: bool,
) -> (Vec<AccountAction>, bool) {
    let (unseen_count, updated_count) = summarize_counts(&subset);
    let heading = format!(
        "{title} ({} unseen, {} updated)",
        unseen_count, updated_count
    );
    let heading_text = if highlight {
        RichText::new(heading.clone())
            .strong()
            .color(group.visuals().warn_fg_color)
    } else {
        RichText::new(heading.clone()).strong()
    };
    let header = egui::CollapsingHeader::new(heading_text)
        // Keep the collapsing state stable even as counts in the title change.
        .id_salt(format!("notification-section-{title}"))
        .default_open(true);

    if subset.is_empty() {
        let response = header.show(group, |section| {
            section.weak(empty_label);
        });
        return (Vec::new(), response.body_returned.is_some() && highlight);
    }

    let mut actions = Vec::new();
    let response = header.show(group, |section| {
        actions.extend(draw_notifications(section, &subset, filter, inflight_done));
    });
    (actions, response.body_returned.is_some() && highlight)
}

fn summarize_counts(items: &[&NotificationItem]) -> (usize, usize) {
    let mut unseen = 0;
    let mut updated = 0;
    for item in items {
        let visual = notification_state(item);
        if item.unread {
            unseen += 1;
        }
        if visual.needs_revisit {
            updated += 1;
        }
    }
    (unseen, updated)
}

struct SectionCounts {
    unseen: usize,
    updated: usize,
}

impl SectionCounts {
    fn new(unseen: usize, updated: usize) -> Self {
        Self { unseen, updated }
    }

    fn bumped_since(&self, previous: &SectionCounts) -> bool {
        self.unseen > previous.unseen || self.updated > previous.updated
    }
}

struct SectionStats {
    inbox: SectionCounts,
    review_requests: SectionCounts,
    mentions: SectionCounts,
    notifications: SectionCounts,
}

fn section_stats(inbox: &InboxSnapshot) -> SectionStats {
    const REVIEW_REQUEST_REASON: &str = "review_requested";
    const MENTION_REASONS: &[&str] = &["mention", "team_mention"];

    let all_notifications: Vec<_> = inbox.notifications.iter().collect();
    let review_requests: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| item.reason == REVIEW_REQUEST_REASON)
        .collect();
    let mentions: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| MENTION_REASONS.contains(&item.reason.as_str()))
        .collect();
    let other: Vec<_> = inbox
        .notifications
        .iter()
        .filter(|item| {
            item.reason != REVIEW_REQUEST_REASON && !MENTION_REASONS.contains(&item.reason.as_str())
        })
        .collect();

    let (inbox_unseen, inbox_updated) = summarize_counts(&all_notifications);
    let (rr_unseen, rr_updated) = summarize_counts(&review_requests);
    let (m_unseen, m_updated) = summarize_counts(&mentions);
    let (o_unseen, o_updated) = summarize_counts(&other);

    SectionStats {
        inbox: SectionCounts::new(inbox_unseen, inbox_updated),
        review_requests: SectionCounts::new(rr_unseen, rr_updated),
        mentions: SectionCounts::new(m_unseen, m_updated),
        notifications: SectionCounts::new(o_unseen, o_updated),
    }
}

// Highlight notifications that churned after the last time we read the thread so
// they do not silently blend into the "seen" palette. GitHub surfaces
// `last_read_at` alongside `unread`, but clients may set `unread` to false while a
// thread continues to evolve.
#[derive(Clone, Copy)]
struct NotificationVisualState {
    seen: bool,
    needs_revisit: bool,
}

fn notification_state(item: &NotificationItem) -> NotificationVisualState {
    let needs_revisit = item
        .last_read_at
        .map(|last_read| item.updated_at > last_read)
        .unwrap_or(false);

    NotificationVisualState {
        // A thread counts as "seen" only if GitHub marks it read and no updates
        // landed after that read timestamp.
        seen: !item.unread && !needs_revisit,
        needs_revisit,
    }
}

fn notification_text(
    ui: &egui::Ui,
    text: impl Into<String>,
    visual: NotificationVisualState,
) -> RichText {
    let mut content = RichText::new(text.into());
    if visual.needs_revisit {
        content = content.color(ui.visuals().warn_fg_color);
    } else if visual.seen {
        content = content.color(ui.visuals().weak_text_color());
    }
    content
}

fn draw_notifications(
    ui: &mut egui::Ui,
    items: &[&NotificationItem],
    filter: &SearchFilter,
    inflight_done: &HashSet<String>,
) -> Vec<AccountAction> {
    let rows: Vec<_> = items
        .iter()
        .copied()
        .filter(|item| filter.matches_any(&[&item.repo, &item.title, &item.reason]))
        .collect();
    if rows.is_empty() {
        ui.weak("No matches for current search.");
        return Vec::new();
    }

    if uses_compact_notifications(ui.available_width()) {
        return draw_notification_cards(ui, &rows, inflight_done);
    }

    draw_notification_table(ui, &rows, inflight_done)
}

fn draw_notification_cards(
    ui: &mut egui::Ui,
    rows: &[&NotificationItem],
    inflight_done: &HashSet<String>,
) -> Vec<AccountAction> {
    let mut actions = Vec::new();

    for item in rows {
        let visual = notification_state(item);
        ui.group(|card| {
            card.vertical(|column| {
                column.horizontal_wrapped(|row| {
                    row.label(notification_text(row, &item.repo, visual));
                    row.separator();
                    row.label(notification_text(
                        row,
                        item.updated_at.format("%Y-%m-%d %H:%M").to_string(),
                        visual,
                    ));
                    if visual.needs_revisit {
                        row.small(
                            RichText::new("Updated")
                                .strong()
                                .color(row.visuals().warn_fg_color),
                        );
                    }
                });

                if let Some(url) = &item.url {
                    let resp =
                        column.hyperlink_to(notification_text(column, &item.title, visual), url);
                    if resp.clicked() {
                        actions.push(AccountAction::Seen(item.thread_id.clone()));
                    }
                } else {
                    let resp = column.label(notification_text(column, &item.title, visual));
                    if resp.clicked() {
                        actions.push(AccountAction::Seen(item.thread_id.clone()));
                    }
                }

                column.small(notification_text(
                    column,
                    format!("Reason: {}", &item.reason),
                    visual,
                ));

                column.horizontal_wrapped(|row| {
                    let busy = inflight_done.contains(&item.thread_id);
                    let already_read = !item.unread && !visual.needs_revisit;

                    if row
                        .add_enabled(!busy && !already_read, egui::Button::new("Mark read"))
                        .clicked()
                    {
                        actions.push(AccountAction::Read(item.thread_id.clone()));
                    }

                    if busy {
                        row.spinner();
                    }
                });
            });
        });
        ui.add_space(8.0);
    }

    actions
}

fn draw_notification_table(
    ui: &mut egui::Ui,
    rows: &[&NotificationItem],
    inflight_done: &HashSet<String>,
) -> Vec<AccountAction> {
    let mut actions = Vec::new();

    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .column(Column::initial(120.0).resizable(true))
                .column(Column::remainder().at_least(140.0))
                .column(Column::initial(130.0).resizable(true))
                .column(Column::initial(100.0))
                .header(20.0, |mut header| {
                    header.col(|ui| {
                        ui.strong("Repository");
                    });
                    header.col(|ui| {
                        ui.strong("Subject");
                    });
                    header.col(|ui| {
                        ui.strong("Updated");
                    });
                    header.col(|ui| {
                        ui.strong("Actions");
                    });
                })
                .body(|mut body| {
                    for item in rows {
                        let visual = notification_state(item);
                        body.row(24.0, |mut row| {
                            row.col(|ui| {
                                ui.label(notification_text(ui, &item.repo, visual));
                            });
                            row.col(|ui| {
                                ui.horizontal(|row_ui| {
                                    let subject = notification_text(row_ui, &item.title, visual);
                                    if let Some(url) = &item.url {
                                        let resp = row_ui.hyperlink_to(subject, url);
                                        if resp.clicked() {
                                            actions
                                                .push(AccountAction::Seen(item.thread_id.clone()));
                                        }
                                    } else {
                                        let resp = row_ui.label(subject);
                                        if resp.clicked() {
                                            actions
                                                .push(AccountAction::Seen(item.thread_id.clone()));
                                        }
                                    }
                                    if visual.needs_revisit {
                                        row_ui.small(
                                            RichText::new("Updated")
                                                .strong()
                                                .color(row_ui.visuals().warn_fg_color),
                                        );
                                    }
                                });
                                ui.small(notification_text(
                                    ui,
                                    format!("Reason: {}", &item.reason),
                                    visual,
                                ));
                            });
                            row.col(|ui| {
                                ui.label(notification_text(
                                    ui,
                                    item.updated_at.format("%Y-%m-%d %H:%M").to_string(),
                                    visual,
                                ));
                            });
                            row.col(|ui| {
                                let busy = inflight_done.contains(&item.thread_id);
                                let already_read = !item.unread && !visual.needs_revisit;

                                if ui
                                    .add_enabled(
                                        !busy && !already_read,
                                        egui::Button::new("Mark read"),
                                    )
                                    .clicked()
                                {
                                    actions.push(AccountAction::Read(item.thread_id.clone()));
                                }

                                // Keep layout width consistent even when disabled.
                                if busy {
                                    ui.spinner();
                                }
                            });
                        });
                    }
                });
        });
    actions
}

fn responsive_accounts_panel_width(window_width: f32) -> f32 {
    (window_width * ACCOUNTS_PANEL_WIDTH_RATIO)
        .clamp(ACCOUNTS_PANEL_MIN_WIDTH, ACCOUNTS_PANEL_MAX_WIDTH)
}

fn tracked_account_heading(
    ui: &egui::Ui,
    account: &AccountState,
    is_selected: bool,
    overview: Option<AccountOverview>,
) -> RichText {
    let has_new = overview
        .map(|overview| overview.new_notifications > 0)
        .unwrap_or(false);
    let mut text = RichText::new(&account.profile.login);
    if is_selected || has_new {
        text = text.strong();
    }
    if has_new && !is_selected {
        text = text.color(ui.visuals().warn_fg_color);
    }
    text
}

fn render_tracked_account_badges(
    ui: &mut egui::Ui,
    overview: Option<AccountOverview>,
    pending: bool,
    has_error: bool,
) {
    if let Some(overview) = overview {
        if overview.new_notifications > 0 {
            ui.small(
                RichText::new(format!("new +{}", overview.new_notifications))
                    .strong()
                    .color(ui.visuals().warn_fg_color),
            );
        }
        ui.small(format!("unseen {}", overview.unseen));
        let updated = if overview.updated > 0 {
            RichText::new(format!("updated {}", overview.updated)).color(ui.visuals().warn_fg_color)
        } else {
            RichText::new(format!("updated {}", overview.updated))
                .color(ui.visuals().weak_text_color())
        };
        ui.small(updated);
    } else if pending {
        ui.small(RichText::new("Loading…").color(ui.visuals().weak_text_color()));
    } else {
        ui.small(RichText::new("No data yet").color(ui.visuals().weak_text_color()));
    }

    if pending {
        ui.small(RichText::new("syncing…").color(ui.visuals().weak_text_color()));
    }
    if has_error {
        ui.small(RichText::new("sync failed").color(ui.visuals().error_fg_color));
    }
}

fn collect_new_notification_ids(
    previous: Option<&InboxSnapshot>,
    next: &InboxSnapshot,
) -> HashSet<String> {
    let Some(previous) = previous else {
        return HashSet::new();
    };

    let known_ids: HashSet<_> = previous
        .notifications
        .iter()
        .map(|item| item.thread_id.as_str())
        .collect();

    next.notifications
        .iter()
        .filter(|item| !known_ids.contains(item.thread_id.as_str()))
        .map(|item| item.thread_id.clone())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AccountOverview {
    new_notifications: usize,
    unseen: usize,
    updated: usize,
}

fn account_overview(account: &AccountState) -> Option<AccountOverview> {
    account.inbox.as_ref().map(|inbox| {
        let stats = section_stats(inbox);
        AccountOverview {
            new_notifications: account.new_notification_ids.len(),
            unseen: stats.inbox.unseen,
            updated: stats.inbox.updated,
        }
    })
}

fn uses_compact_account_rows(available_width: f32) -> bool {
    available_width < COMPACT_ACCOUNT_ROW_WIDTH
}

fn uses_stacked_account_header(available_width: f32) -> bool {
    available_width < STACKED_ACCOUNT_HEADER_WIDTH
}

fn uses_compact_notifications(available_width: f32) -> bool {
    available_width < COMPACT_NOTIFICATION_WIDTH
}

// -----------------------------------------------------------------------------
// Supporting structs
// -----------------------------------------------------------------------------

#[allow(dead_code)]
enum AccountAction {
    Done(String),
    Seen(String),
    Read(String),
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum SectionKind {
    Inbox,
    ReviewRequests,
    Mentions,
    Notifications,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AccountViewMode {
    Inbox,
    Grouped,
}

#[derive(Default)]
struct AccountForm {
    login: String,
    token: String,
    form_error: Option<String>,
}

struct BatchRefreshScheduler {
    interval: Duration,
    last_run: Option<Instant>,
}

impl BatchRefreshScheduler {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_run: None,
        }
    }

    fn should_trigger(&self) -> bool {
        match self.last_run {
            None => true,
            Some(instant) => instant.elapsed() >= self.interval,
        }
    }

    fn mark_triggered(&mut self) {
        self.last_run = Some(Instant::now());
    }
}

// -----------------------------------------------------------------------------
// Search filtering
// -----------------------------------------------------------------------------

struct SearchFilter {
    needle: Option<String>,
}

impl SearchFilter {
    fn new(raw: &str) -> Self {
        let trimmed = raw.trim();
        let needle = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_lowercase())
        };
        Self { needle }
    }

    fn matches_any(&self, fields: &[&str]) -> bool {
        match &self.needle {
            None => true,
            Some(needle) => fields
                .iter()
                .any(|field| field.to_lowercase().contains(needle)),
        }
    }
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, NaiveDateTime, Utc};
    use eframe::egui;
    use eframe::egui::collapsing_header::CollapsingState;

    fn parse_utc(ts: &str) -> DateTime<Utc> {
        NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc()
    }

    fn notif(thread_id: &str, reason: &str, unread: bool, updated: &str) -> NotificationItem {
        NotificationItem {
            thread_id: thread_id.to_string(),
            repo: "acme/repo".into(),
            title: "Title".into(),
            url: None,
            reason: reason.into(),
            updated_at: parse_utc(updated),
            last_read_at: None,
            unread,
        }
    }

    fn inbox_with_notifications(notifications: Vec<NotificationItem>) -> InboxSnapshot {
        InboxSnapshot {
            notifications,
            review_requests: Vec::new(),
            mentions: Vec::new(),
            recent_reviews: Vec::new(),
            fetched_at: Utc::now(),
        }
    }

    fn dummy_profile() -> GitHubAccount {
        GitHubAccount {
            login: "user".into(),
            token: "token".into(),
        }
    }

    fn make_account(login: &str) -> AccountState {
        AccountState::new(GitHubAccount {
            login: login.into(),
            token: "token".into(),
        })
    }

    fn app_with_accounts(logins: &[&str]) -> ReminderApp {
        ReminderApp {
            account_form: AccountForm::default(),
            accounts: logins.iter().map(|login| make_account(login)).collect(),
            selected_account_login: None,
            secret_store: None,
            storage_warning: None,
            global_error: None,
            auto_refresh: BatchRefreshScheduler::new(Duration::from_secs(1)),
        }
    }

    #[test]
    fn section_stats_groups_by_reason() {
        let inbox = inbox_with_notifications(vec![
            notif("1", "review_requested", true, "2024-01-01 00:00:00"),
            notif("2", "mention", true, "2024-01-01 00:00:00"),
            notif("3", "subscribed", false, "2024-01-01 00:00:00"),
        ]);
        let stats = section_stats(&inbox);
        assert_eq!(stats.inbox.unseen, 2);
        assert_eq!(stats.review_requests.unseen, 1);
        assert_eq!(stats.mentions.unseen, 1);
        assert_eq!(stats.notifications.unseen, 0);
    }

    #[test]
    fn new_accounts_start_in_inbox_view() {
        let account = AccountState::new(dummy_profile());
        assert_eq!(account.view_mode, AccountViewMode::Inbox);
    }

    #[test]
    fn section_counts_bumped_on_unseen_increase() {
        let old = SectionCounts::new(1, 0);
        let new = SectionCounts::new(2, 0);
        assert!(new.bumped_since(&old));
    }

    #[test]
    fn section_counts_not_bumped_when_same() {
        let old = SectionCounts::new(1, 1);
        let new = SectionCounts::new(1, 1);
        assert!(!new.bumped_since(&old));
    }

    #[test]
    fn search_filter_matches_case_insensitive() {
        let filter = SearchFilter::new("Repo");
        assert!(filter.matches_any(&["my/repo"]));
        assert!(!filter.matches_any(&["other/project"]));
    }

    #[test]
    fn batch_scheduler_triggers_after_interval() {
        let mut scheduler = BatchRefreshScheduler::new(Duration::from_secs(1));
        assert!(scheduler.should_trigger());
        scheduler.mark_triggered();
        scheduler.last_run = Some(Instant::now() - Duration::from_secs(2));
        assert!(scheduler.should_trigger());
    }

    #[test]
    fn accounts_panel_width_is_clamped() {
        assert_eq!(
            responsive_accounts_panel_width(300.0),
            ACCOUNTS_PANEL_MIN_WIDTH
        );
        assert_eq!(
            responsive_accounts_panel_width(2_000.0),
            ACCOUNTS_PANEL_MAX_WIDTH
        );
    }

    #[test]
    fn narrow_layout_helpers_switch_at_breakpoints() {
        assert!(uses_compact_account_rows(COMPACT_ACCOUNT_ROW_WIDTH - 1.0));
        assert!(uses_stacked_account_header(
            STACKED_ACCOUNT_HEADER_WIDTH - 1.0
        ));
        assert!(uses_compact_notifications(COMPACT_NOTIFICATION_WIDTH - 1.0));
        assert!(!uses_compact_notifications(
            COMPACT_NOTIFICATION_WIDTH + 1.0
        ));
    }

    #[test]
    fn ensure_selected_account_falls_back_to_first_account() {
        let mut app = app_with_accounts(&["alpha", "beta"]);
        app.selected_account_login = Some("missing".into());

        app.ensure_selected_account();

        assert_eq!(app.selected_account_login.as_deref(), Some("alpha"));
    }

    #[test]
    fn collect_new_notification_ids_ignores_initial_sync() {
        let next =
            inbox_with_notifications(vec![notif("1", "subscribed", true, "2024-01-01 00:00:00")]);

        assert!(collect_new_notification_ids(None, &next).is_empty());
    }

    #[test]
    fn collect_new_notification_ids_detects_added_threads() {
        let previous = inbox_with_notifications(vec![
            notif("1", "subscribed", true, "2024-01-01 00:00:00"),
            notif("2", "mention", true, "2024-01-01 00:00:00"),
        ]);
        let next = inbox_with_notifications(vec![
            notif("2", "mention", true, "2024-01-01 00:00:00"),
            notif("3", "review_requested", true, "2024-01-01 00:00:00"),
        ]);

        let new_ids = collect_new_notification_ids(Some(&previous), &next);

        assert_eq!(new_ids.len(), 1);
        assert!(new_ids.contains("3"));
    }

    #[test]
    fn account_overview_reports_sidebar_counts() {
        let mut account = make_account("alpha");
        account.inbox = Some(inbox_with_notifications(vec![
            notif("1", "subscribed", true, "2024-01-01 00:00:00"),
            notif("2", "subscribed", false, "2024-01-02 00:00:00"),
        ]));
        account.new_notification_ids = [String::from("2"), String::from("3")].into_iter().collect();
        if let Some(inbox) = &mut account.inbox {
            inbox.notifications[1].last_read_at = Some(parse_utc("2024-01-01 00:00:00"));
        }

        let overview = account_overview(&account).expect("overview should exist");

        assert_eq!(overview.new_notifications, 2);
        assert_eq!(overview.unseen, 1);
        assert_eq!(overview.updated, 1);
    }

    #[test]
    fn clear_new_notifications_resets_sidebar_badge_state() {
        let mut account = make_account("alpha");
        account.new_notification_ids = [String::from("1")].into_iter().collect();

        account.clear_new_notifications();

        assert!(account.new_notification_ids.is_empty());
    }

    #[test]
    fn notification_state_detects_revisit() {
        let mut item = notif("1", "subscribed", false, "2024-01-02 00:00:00");
        item.last_read_at = Some(parse_utc("2024-01-01 00:00:00"));
        let visual = notification_state(&item);
        assert!(visual.needs_revisit);
        assert!(!visual.seen);
    }

    #[test]
    fn highlight_clears_after_rendering_section() {
        let ctx = egui::Context::default();
        let mut account = AccountState::new(dummy_profile());
        account.inbox = Some(inbox_with_notifications(vec![notif(
            "t1",
            "subscribed",
            true,
            "2024-01-01 00:00:00",
        )]));
        account.highlights.insert(SectionKind::Notifications);
        let filter = SearchFilter::new("");

        ctx.begin_pass(Default::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            let _ = render_bucket_sections(ui, &mut account, &filter);
        });
        let _ = ctx.end_pass();

        assert!(
            !account.highlights.contains(&SectionKind::Notifications),
            "Highlight should clear after section is rendered"
        );
    }

    #[test]
    fn collapsing_header_state_persists_across_frames() {
        let ctx = egui::Context::default();
        let mut account = AccountState::new(dummy_profile());
        account.inbox = Some(inbox_with_notifications(vec![notif(
            "t1",
            "subscribed",
            true,
            "2024-01-01 00:00:00",
        )]));
        let filter = SearchFilter::new("");

        // Frame 1: render and manually collapse the notifications section.
        ctx.begin_pass(Default::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            let _ = render_bucket_sections(ui, &mut account, &filter);
        });
        let id = egui::Id::new("notification-section-Notifications");
        let mut state = CollapsingState::load_with_default_open(&ctx, id, true);
        state.set_open(false);
        state.store(&ctx);
        let _ = ctx.end_pass();

        // Frame 2: re-render; section should remain collapsed because ID is stable.
        ctx.begin_pass(Default::default());
        let mut stayed_collapsed = true;
        egui::CentralPanel::default().show(&ctx, |ui| {
            let response = render_bucket_sections(ui, &mut account, &filter);
            let state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
            stayed_collapsed = !state.is_open();
            assert!(response.is_empty(), "Rendering should not trigger actions");
        });
        let _ = ctx.end_pass();

        assert!(
            stayed_collapsed,
            "Collapse state should persist across frames"
        );
    }
}
