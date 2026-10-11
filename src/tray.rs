// SPDX-License-Identifier: Apache-2.0
//! Pip in the menu bar: napping when idle, running while tokens stream
//! (faster with throughput), sprinting during a stampede. The menu shows
//! today's count, the per-model breakdown, the portal, S3 and quit.

use crate::app::{ago, fmt_compact, fmt_thousands, App};
use crate::live::PipState;
use crate::{config, ledger, login, portal, s3sync};
use chrono::Local;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_W: u32 = 96;
const ICON_H: u32 = 72;
const RUN_FRAMES: [&[u8]; 4] = [
    include_bytes!("../assets/tray/run0.rgba"),
    include_bytes!("../assets/tray/run1.rgba"),
    include_bytes!("../assets/tray/run2.rgba"),
    include_bytes!("../assets/tray/run3.rgba"),
];
const SLEEP_FRAME: &[u8] = include_bytes!("../assets/tray/sleep.rgba");

/// Icon updates: ~16 per second; menu text: every 2 seconds.
const ANIM_EVERY: Duration = Duration::from_millis(60);
const REFRESH_EVERY_TICKS: u32 = 33;

enum UserEvent {
    Menu(MenuEvent),
    Anim,
    Refresh,
}

struct Ui {
    tray: TrayIcon,
    run: Vec<Icon>,
    sleep: Icon,
    pip_line: MenuItem,
    today: MenuItem,
    requests: MenuItem,
    by_model: Submenu,
    open_portal: MenuItem,
    open_data: MenuItem,
    edit_config: MenuItem,
    sync: MenuItem,
    s3_status: MenuItem,
    proxy_status: MenuItem,
    login: CheckMenuItem,
    about: MenuItem,
    quit: MenuItem,
    model_lines: Vec<String>,
    last_title: String,
    // Animation state
    shown: Option<usize>, // None = sleeping frame
    frame: usize,
    since_frame: Duration,
    last_anim: Instant,
    state: PipState,
}

pub fn run(app: Arc<App>) -> ! {
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        // Menu bar only: no Dock icon, no app menu.
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let menu_proxy = Mutex::new(event_loop.create_proxy());
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.lock().send_event(UserEvent::Menu(e));
    }));
    let tick_proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        let mut n = 0u32;
        loop {
            std::thread::sleep(ANIM_EVERY);
            n = n.wrapping_add(1);
            let ev = if n.is_multiple_of(REFRESH_EVERY_TICKS) {
                UserEvent::Refresh
            } else {
                UserEvent::Anim
            };
            if tick_proxy.send_event(ev).is_err() {
                break;
            }
        }
    });

    let mut ui: Option<Ui> = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => match Ui::new(&app) {
                Ok(mut u) => {
                    u.refresh(&app);
                    ui = Some(u);
                }
                Err(e) => {
                    log::error!("could not create the menu bar item: {e:#}");
                    std::process::exit(1);
                }
            },
            Event::UserEvent(UserEvent::Anim) => {
                if let Some(u) = ui.as_mut() {
                    u.animate(&app);
                }
            }
            Event::UserEvent(UserEvent::Refresh) => {
                if let Some(u) = ui.as_mut() {
                    u.animate(&app);
                    u.refresh(&app);
                }
            }
            Event::UserEvent(UserEvent::Menu(e)) => {
                if let Some(u) = ui.as_mut() {
                    u.on_click(&app, e);
                }
            }
            _ => {}
        }
    })
}

fn icon(bytes: &[u8]) -> anyhow::Result<Icon> {
    Ok(Icon::from_rgba(bytes.to_vec(), ICON_W, ICON_H)?)
}

/// What Pip says in the first line of the menu.
pub fn pip_says(state: PipState, tps: f64) -> String {
    match state {
        PipState::Idle => "Pip is napping. Nothing to count. Nap time!".into(),
        PipState::Processing => format!(
            "Nom nom nom… {} tokens/sec!",
            fmt_thousands(tps.round() as u64)
        ),
        PipState::Stampede => format!(
            "Holy tokens! We've got a stampede! ({} tokens/sec)",
            fmt_thousands(tps.round() as u64)
        ),
    }
}

impl Ui {
    fn new(app: &Arc<App>) -> anyhow::Result<Ui> {
        let pip_line = MenuItem::new("Pip is waking up…", false, None);
        let today = MenuItem::new("Today: —", false, None);
        let requests = MenuItem::new("Requests today: —", false, None);
        let by_model = Submenu::new("Today by model", true);
        let open_portal = MenuItem::new("Open Portal…", true, None);
        let open_data = MenuItem::new("Open Data Folder", true, None);
        let edit_config = MenuItem::new("Edit Config…", true, None);
        let s3_enabled = app.config.read().s3.enabled;
        let sync = MenuItem::new("Sync to S3 Now", s3_enabled, None);
        let s3_status = MenuItem::new("S3: off", false, None);
        let proxy_status = MenuItem::new("Proxy: starting…", false, None);
        let login = CheckMenuItem::new(
            "Launch at Login",
            cfg!(target_os = "macos"),
            login::is_enabled(),
            None,
        );
        let about = MenuItem::new(
            format!("About Toktail {}…", env!("CARGO_PKG_VERSION")),
            true,
            None,
        );
        let quit = MenuItem::new("Quit Toktail", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &pip_line,
            &PredefinedMenuItem::separator(),
            &today,
            &requests,
            &by_model,
            &PredefinedMenuItem::separator(),
            &open_portal,
            &open_data,
            &edit_config,
            &PredefinedMenuItem::separator(),
            &sync,
            &s3_status,
            &proxy_status,
            &PredefinedMenuItem::separator(),
            &login,
            &about,
            &quit,
        ])?;

        let run = RUN_FRAMES
            .iter()
            .map(|b| icon(b))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let sleep = icon(SLEEP_FRAME)?;
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Toktail");
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(sleep.clone());
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(sleep.clone());
        let tray = builder.build()?;

        Ok(Ui {
            tray,
            run,
            sleep,
            pip_line,
            today,
            requests,
            by_model,
            open_portal,
            open_data,
            edit_config,
            sync,
            s3_status,
            proxy_status,
            login,
            about,
            quit,
            model_lines: Vec::new(),
            last_title: String::new(),
            shown: None,
            frame: 0,
            since_frame: Duration::ZERO,
            last_anim: Instant::now(),
            state: PipState::Idle,
        })
    }

    fn set_icon(&self, icon: &Icon) {
        #[cfg(target_os = "macos")]
        let r = self.tray.set_icon_templated(Some(icon.clone()));
        #[cfg(not(target_os = "macos"))]
        let r = self.tray.set_icon(Some(icon.clone()));
        if let Err(e) = r {
            log::debug!("tray icon: {e}");
        }
    }

    /// Advances Pip's run cycle. Frame time shrinks as throughput grows.
    fn animate(&mut self, app: &Arc<App>) {
        let (snap, state) = app.pip(10);
        self.state = state;
        let now = Instant::now();
        let dt = now - self.last_anim;
        self.last_anim = now;
        let period = match state {
            PipState::Idle => {
                if self.shown.is_some() {
                    self.set_icon(&self.sleep);
                    self.shown = None;
                }
                return;
            }
            PipState::Processing => Duration::from_millis(
                (200.0 - snap.tokens_per_sec.min(200.0) * 0.6).max(80.0) as u64,
            ),
            PipState::Stampede => Duration::from_millis(55),
        };
        self.since_frame += dt;
        if self.shown.is_none() || self.since_frame >= period {
            self.since_frame = Duration::ZERO;
            self.frame = (self.frame + 1) % self.run.len();
            self.set_icon(&self.run[self.frame]);
            self.shown = Some(self.frame);
        }
    }

    fn refresh(&mut self, app: &Arc<App>) {
        let (snap, state) = app.pip(10);
        self.pip_line.set_text(pip_says(state, snap.tokens_per_sec));

        let day = Local::now().date_naive();
        let per_model = app.ledger.with_data(|d| ledger::day_by_model(d, day));
        let (inp, out, req) = per_model.iter().fold((0, 0, 0), |a, m| {
            (
                a.0 + m.totals.input,
                a.1 + m.totals.output,
                a.2 + m.totals.requests,
            )
        });
        self.today.set_text(format!(
            "Today: {} out · {} in",
            fmt_thousands(out),
            fmt_thousands(inp)
        ));
        self.requests
            .set_text(format!("Requests today: {}", fmt_thousands(req)));

        let lines: Vec<String> = if per_model.is_empty() {
            vec!["No requests yet today".into()]
        } else {
            per_model
                .iter()
                .map(|m| {
                    format!(
                        "{} — {} out · {} in · {} req",
                        m.model,
                        fmt_compact(m.totals.output),
                        fmt_compact(m.totals.input),
                        fmt_thousands(m.totals.requests)
                    )
                })
                .collect()
        };
        // Rebuild only when something changed, so an open menu doesn't flicker.
        if lines != self.model_lines {
            while self.by_model.remove_at(0).is_some() {}
            for l in &lines {
                let _ = self.by_model.append(&MenuItem::new(l, false, None));
            }
            self.model_lines = lines;
        }

        let (show_count, s3_enabled) = {
            let c = app.config.read();
            (c.app.show_count_in_menu_bar, c.s3.enabled)
        };
        let title = if show_count {
            fmt_compact(out)
        } else {
            String::new()
        };
        if title != self.last_title {
            self.tray.set_title(if title.is_empty() {
                None
            } else {
                Some(title.clone())
            });
            let _ = self.tray.set_tooltip(Some(format!(
                "Toktail — {} tokens out today. Every token leaves a trail.",
                fmt_thousands(out)
            )));
            self.last_title = title;
        }

        let st = app.status.read().clone();
        let s3_text = if !s3_enabled {
            "S3: off (see Edit Config…)".to_string()
        } else if st.s3.in_progress {
            "S3: syncing…".to_string()
        } else if let Some(err) = &st.s3.last_error {
            let short: String = err.chars().take(70).collect();
            format!("S3: error — {short}")
        } else if let Some(t) = st.s3.last_ok {
            format!("S3: synced {}", ago(t))
        } else {
            "S3: waiting for first sync".to_string()
        };
        self.s3_status.set_text(s3_text);
        self.proxy_status.set_text(if st.proxy_ok {
            format!("✓ {}", st.proxy_msg)
        } else {
            format!(
                "⚠ {}",
                if st.proxy_msg.is_empty() {
                    "Proxy starting…"
                } else {
                    &st.proxy_msg
                }
            )
        });
    }

    fn on_click(&mut self, app: &Arc<App>, e: MenuEvent) {
        let id = e.id();
        if id == self.open_portal.id() {
            open(&app.portal_url(), false);
        } else if id == self.open_data.id() {
            open(&app.ledger.dir().display().to_string(), false);
        } else if id == self.edit_config.id() {
            open(&app.paths.config_file.display().to_string(), true);
        } else if id == self.about.id() {
            open(portal::REPOSITORY, false);
        } else if id == self.sync.id() {
            app.sync_now.notify_one();
            self.s3_status.set_text("S3: syncing…");
        } else if id == self.login.id() {
            let want = self.login.is_checked();
            match login::set_enabled(want) {
                Ok(()) => {
                    let mut c = app.config.write();
                    c.app.launch_at_login = want;
                    if let Err(e) = config::save(&app.paths.config_file, &c) {
                        log::warn!("could not save config: {e:#}");
                    }
                }
                Err(err) => {
                    log::error!("launch at login: {err:#}");
                    self.login.set_checked(!want);
                }
            }
        } else if id == self.quit.id() {
            self.quit.set_text("Pip is tidying up… (final S3 sync)");
            app.rt.block_on(s3sync::final_sync(app));
            std::process::exit(0);
        }
    }
}

fn open(target: &str, as_text: bool) {
    #[cfg(target_os = "macos")]
    let r = {
        let mut c = std::process::Command::new("open");
        if as_text {
            c.arg("-t");
        }
        c.arg(target).spawn()
    };
    #[cfg(not(target_os = "macos"))]
    let r = {
        let _ = as_text;
        std::process::Command::new("xdg-open").arg(target).spawn()
    };
    if let Err(e) = r {
        log::warn!("could not open {target}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_the_right_size() {
        for f in RUN_FRAMES.iter().chain([&SLEEP_FRAME]) {
            assert_eq!(f.len() as u32, ICON_W * ICON_H * 4);
            assert!(f.chunks(4).any(|p| p[3] > 200), "frame has visible pixels");
        }
    }

    #[test]
    fn pip_lines() {
        assert!(pip_says(PipState::Idle, 0.0).contains("Nap time"));
        assert_eq!(
            pip_says(PipState::Processing, 2847.2),
            "Nom nom nom… 2,847 tokens/sec!"
        );
        assert!(pip_says(PipState::Stampede, 900.0).starts_with("Holy tokens!"));
    }
}
