//! simlab-engine: SimLab's local engine.
//!
//!   simlab-engine serve [--port 7311] [--data <dir>] [--idle-exit <seconds>]
//!   simlab-engine mcp   [--port 7311]      MCP over stdio (for Claude)
//!   simlab-engine check                    print the system check

mod api;
mod db;
mod jobs;
mod mcp;
mod models;
mod solve;
mod state;
mod system;
mod wind;
mod calculix;

use std::sync::Arc;

pub const DEFAULT_PORT: u16 = 7311;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("serve");
    let port: u16 = flag(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    match cmd {
        "mcp" => mcp::stdio(port),
        "check" => {
            let app = make_app(&args)?.0;
            for c in system::run_checks(&app) {
                println!("{:9} {:42} {}", c.status, c.label, c.detail);
            }
            Ok(())
        }
        "serve" => serve(&args, port),
        "--version" | "version" => {
            println!("simlab-engine {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => anyhow::bail!("unknown command {other}; use serve, mcp or check"),
    }
}

type Rx = tokio::sync::mpsc::UnboundedReceiver<String>;
type Queues = (Rx, Rx, Rx);

fn make_app(args: &[String]) -> anyhow::Result<(Arc<state::App>, Queues)> {
    let data = flag(args, "--data").map(std::path::PathBuf::from).unwrap_or_else(state::default_data_dir);
    let (gtx, grx) = tokio::sync::mpsc::unbounded_channel();
    let (ctx, crx) = tokio::sync::mpsc::unbounded_channel();
    let (qtx, qrx) = tokio::sync::mpsc::unbounded_channel();
    Ok((state::App::new(data, gtx, ctx, qtx)?, (grx, crx, qrx)))
}

/// Leaves the process, tidying up what SimLab started outside it.
fn quit(app: &state::App) -> ! {
    system::shutdown(app);
    std::process::exit(0)
}

fn serve(args: &[String], port: u16) -> anyhow::Result<()> {
    let (app, (grx, crx, qrx)) = make_app(args)?;
    let idle_exit: Option<u64> = flag(args, "--idle-exit").and_then(|s| s.parse().ok());
    let threads = app.settings().cpu_threads.max(1);
    // solver threads (rayon) on the performance cores; tokio stays small
    let _ = rayon::ThreadPoolBuilder::new().num_threads(threads).build_global();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| anyhow::anyhow!("port {port} is busy ({e}); is SimLab already open?"))?;
        eprintln!("SimLab engine on http://127.0.0.1:{port} · data in {}", app.data.display());
        {
            let app = app.clone();
            tokio::task::spawn_blocking(move || {
                system::refresh(&app);
                // OpenFOAM/CalculiX containers an earlier engine left running
                system::kill_stale_containers(&app);
            });
        }
        tokio::spawn(jobs::worker(app.clone(), grx));
        tokio::spawn(jobs::worker(app.clone(), crx));
        tokio::spawn(jobs::worker(app.clone(), qrx));
        // started by SimLab.app: leave when the app goes, even if it crashed
        // or was force-quit (the window then never says goodbye)
        if args.iter().any(|a| a == "--exit-with-parent") {
            let parent = std::os::unix::process::parent_id();
            let app = app.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if std::os::unix::process::parent_id() != parent {
                    eprintln!("SimLab.app is gone, exiting");
                    quit(&app);
                }
            });
        }
        if let Some(secs) = idle_exit {
            let app = app.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    let idle = app.last_activity.lock().unwrap().elapsed().as_secs();
                    let busy = !app.stops.lock().unwrap().is_empty();
                    if idle > secs && !busy && app.events.receiver_count() == 0 {
                        eprintln!("idle for {idle} s, exiting");
                        let app = app.clone();
                        let _ = tokio::task::spawn_blocking(move || quit(&app)).await;
                    }
                }
            });
        }
        let router = api::router(app.clone());
        let app2 = app.clone();
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                // Ctrl-C in a terminal, or SIGTERM from SimLab.app quitting
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                // open live-event streams never end on their own, so leave now;
                // SQLite (WAL) is safe, and unfinished runs show as stopped
                let _ = tokio::task::spawn_blocking(move || quit(&app2)).await;
            })
            .await?;
        anyhow::Ok(())
    })
}
