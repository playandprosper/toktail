//! Menu bar item: today's count, per-model breakdown, portal, S3, quit.

use crate::app::{ago, fmt_compact, fmt_thousands, App};
use crate::{config, ledger, login, s3sync};
use chrono::Local;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

enum UserEvent {
    Menu(MenuEvent),
    Tick,
}

struct Ui {
    tray: TrayIcon,
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
    quit: MenuItem,
    model_lines: Vec<String>,
    last_title: String,
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
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        if tick_proxy.send_event(UserEvent::Tick).is_err() {
            break;
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
            Event::UserEvent(UserEvent::Tick) => {
                if let Some(u) = ui.as_mut() {
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

impl Ui {
    fn new(app: &Arc<App>) -> anyhow::Result<Ui> {
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
        let quit = MenuItem::new("Quit TokenLedger", true, None);

        let menu = Menu::new();
        menu.append_items(&[
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
            &quit,
        ])?;

        let (rgba, w, h) = icon_rgba();
        let icon = Icon::from_rgba(rgba, w, h)?;
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("TokenLedger");
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(icon);
        let tray = builder.build()?;

        Ok(Ui {
            tray,
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
            quit,
            model_lines: Vec::new(),
            last_title: String::new(),
        })
    }

    fn refresh(&mut self, app: &Arc<App>) {
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
                "TokenLedger — {} tokens out today",
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
            self.quit.set_text("Quitting… (final S3 sync)");
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

/// A 36×36 template icon (three rising bars). Black + alpha only, so macOS
/// tints it for light/dark menu bars.
fn icon_rgba() -> (Vec<u8>, u32, u32) {
    let (w, h) = (36u32, 36u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let bars = [(5u32, 14u32), (15, 22), (25, 30)]; // (x, height)
    for (x0, height) in bars {
        for y in (h - 3 - height)..(h - 3) {
            for x in x0..x0 + 7 {
                let i = ((y * w + x) * 4) as usize;
                px[i + 3] = 255;
            }
        }
    }
    (px, w, h)
}
