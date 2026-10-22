#![cfg_attr(all(windows, not(feature = "console")), windows_subsystem = "windows")]

use launcherlauncher::{gui, theme};
use std::sync::Arc;

fn main() -> eframe::Result {
    #[cfg(unix)]
    unsafe {
        // Do not panic when a CLI pipeline closes early (`--list | head`).
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a.starts_with("--")).unwrap_or(false) {
        std::process::exit(launcherlauncher::cli::run(&args));
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([920.0, 570.0])
            .with_min_inner_size([920.0, 570.0])
            .with_max_inner_size([920.0, 570.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_title("LAUNCHER LAUNCHER // KEYGEN")
            .with_icon(Arc::new(theme::make_icon())),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "LauncherLauncher",
        options,
        Box::new(|cc| Ok(Box::new(gui::App::new(cc)))),
    )
}
