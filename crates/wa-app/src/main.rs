//! `whatsapp-cosmic`: native WhatsApp client for the COSMIC desktop.
//!
//! Flags:
//! - `--fake`        run against the scripted demo backend with a throwaway database
//! - `--background`  start hidden with only the tray icon

mod app;
mod audio;
mod backend_sub;
mod config;
mod desktop;
mod pages;
mod widgets;

use std::path::PathBuf;

use cosmic::iced::Size;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::app::{App, Flags};
use crate::backend_sub::BackendConfig;
use crate::config::Config;

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    fake: bool,
    background: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    for arg in args {
        match arg.as_str() {
            "--fake" => parsed.fake = true,
            "--background" => parsed.background = true,
            "-h" | "--help" => {
                return Err("usage: whatsapp-cosmic [--fake] [--background]".to_owned());
            }
            other => {
                return Err(format!(
                    "unknown argument: {other}\nusage: whatsapp-cosmic [--fake] [--background]"
                ));
            }
        }
    }
    Ok(parsed)
}

/// Logs to `~/.local/state/whatsapp-cosmic/log/` (daily files) and stderr.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let file = wa_core::paths::state_dir().ok().and_then(|dir| {
        let dir = dir.join("log");
        std::fs::create_dir_all(&dir).ok()?;
        Some(tracing_appender::rolling::daily(dir, "whatsapp-cosmic.log"))
    });
    match file {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(stderr)
                .with(file_layer)
                .try_init();
            Some(guard)
        }
        None => {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(stderr)
                .try_init();
            None
        }
    }
}

fn backend_config(fake: bool) -> anyhow::Result<(BackendConfig, Option<tempfile::TempDir>)> {
    if fake {
        let dir = tempfile::Builder::new()
            .prefix("whatsapp-cosmic-fake-")
            .tempdir()?;
        let base: PathBuf = dir.path().to_path_buf();
        let media = base.join("media");
        std::fs::create_dir_all(&media)?;
        let config = BackendConfig {
            fake: true,
            app_db: base.join("app.db"),
            session_db: base.join("session.db"),
            media_dir: media,
        };
        return Ok((config, Some(dir)));
    }
    let config = BackendConfig {
        fake: false,
        app_db: wa_core::paths::app_db()?,
        session_db: wa_core::paths::session_db()?,
        media_dir: wa_core::paths::media_dir()?,
    };
    Ok((config, None))
}

fn main() -> std::process::ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("{usage}");
            return std::process::ExitCode::from(2);
        }
    };
    let _log_guard = init_logging();

    let (backend, temp_dir) = match backend_config(args.fake) {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(?err, "cannot prepare data directories");
            eprintln!("whatsapp-cosmic: cannot prepare data directories: {err:#}");
            return std::process::ExitCode::FAILURE;
        }
    };
    tracing::info!(fake = args.fake, background = args.background, db = %backend.app_db.display(), "starting");

    let config_handler = Config::handler();
    let config = Config::load(config_handler.as_ref());

    let settings = cosmic::app::Settings::default()
        .theme(config.appearance.theme())
        .size(Size::new(1100.0, 760.0))
        .size_limits(
            cosmic::iced::core::layout::Limits::NONE
                .min_width(420.0)
                .min_height(480.0),
        )
        .exit_on_close(false)
        .no_main_window(args.background);

    let flags = Flags {
        backend,
        config,
        config_handler,
        temp_dir,
    };
    match cosmic::app::run::<App>(settings, flags) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(?err, "application error");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse_args(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn parses_flags() {
        assert_eq!(args(&[]), Ok(Args::default()));
        assert_eq!(
            args(&["--fake"]),
            Ok(Args {
                fake: true,
                background: false
            })
        );
        assert_eq!(
            args(&["--background", "--fake"]),
            Ok(Args {
                fake: true,
                background: true
            })
        );
        assert!(args(&["--bogus"]).is_err());
    }
}
