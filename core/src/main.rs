use clap::{Parser, Subcommand};
use cortex::engine::Engine;
use cortex::engine::api::{ApiEngine, HttpTransport};
use cortex::engine::claude::ClaudeEngine;
use cortex::engine::fake::FakeEngine;
use cortex::event::Event;
use cortex::mission::{self, Budgets, Outcome};
use cortex::model::{EngineKind, Profile, Roles, find, load_profiles};
use cortex::store::{MissionId, Store};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

/// Cortex: describe what you want; it builds it and proves it works.
#[derive(Parser)]
#[command(name = "cortex", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check that this machine can run Cortex safely.
    Doctor,
    /// Start a mission and work it until it is done or stuck.
    Run {
        /// The git repository to work on. Its checked-out branch is never moved.
        #[arg(long)]
        repo: PathBuf,
        /// The checks that decide when the work is done.
        #[arg(long)]
        checks: PathBuf,
        /// Which profile does the work (see `profiles.toml`); defaults to claude-cli-sonnet.
        #[arg(long)]
        worker: Option<String>,
        /// Script for the fake engine.
        #[arg(long)]
        fake_script: Option<PathBuf>,
        /// What you want, in plain words.
        request: String,
    },
    /// Continue a mission after a crash, restart or pause.
    Resume {
        mission: String,
        /// Which profile does the work (see `profiles.toml`); defaults to claude-cli-sonnet.
        #[arg(long)]
        worker: Option<String>,
        /// Script for the fake engine.
        #[arg(long)]
        fake_script: Option<PathBuf>,
    },
    /// Add an instruction; the next session's brief includes it.
    Steer { mission: String, text: String },
    /// Stop the running session. With text, re-plan with it; without, pause.
    Stop {
        mission: String,
        text: Option<String>,
    },
    /// Show where a mission stands and where its result is.
    Status { mission: String },
    /// Run the bench prompts as missions and write a results file.
    Bench {
        /// Which profile does the work (see `profiles.toml`); defaults to claude-cli-sonnet.
        #[arg(long)]
        worker: Option<String>,
        /// Run only this prompt, e.g. `03-roman`.
        #[arg(long)]
        only: Option<String>,
        /// The bench folder; results go in its `results/`.
        #[arg(long, default_value = "bench")]
        dir: PathBuf,
    },
    /// Serves the sandboxed tools to an engine over stdio (used by engines).
    #[command(hide = true)]
    Mcp {
        #[arg(long)]
        worktree: PathBuf,
        #[arg(long)]
        checks: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => Ok(ExitCode::SUCCESS),
        Some(Cmd::Doctor) => Ok(doctor()),
        Some(Cmd::Run {
            repo,
            checks,
            worker,
            fake_script,
            request,
        }) => run(&repo, &checks, worker, fake_script, &request),
        Some(Cmd::Resume {
            mission,
            worker,
            fake_script,
        }) => resume(&MissionId(mission), worker, fake_script),
        Some(Cmd::Steer { mission, text }) => open_store()
            .and_then(|s| mission::steer(&s, &MissionId(mission), &text))
            .map(|()| ExitCode::SUCCESS),
        Some(Cmd::Stop { mission, text }) => open_store()
            .and_then(|s| mission::stop(&s, &MissionId(mission), text.as_deref()))
            .map(|()| ExitCode::SUCCESS),
        Some(Cmd::Status { mission }) => status(&MissionId(mission)),
        Some(Cmd::Bench { worker, only, dir }) => bench(worker, only.as_deref(), &dir),
        Some(Cmd::Mcp { worktree, checks }) => {
            cortex::mcp::serve(worktree, checks).map(|()| ExitCode::SUCCESS)
        }
    };
    result.unwrap_or_else(|e| {
        eprintln!("cortex: {e}");
        ExitCode::FAILURE
    })
}

fn doctor() -> ExitCode {
    match cortex::sandbox::self_test() {
        Ok(()) => {
            println!("sandbox: ok (bubblewrap + gVisor; secret unreadable; no network)");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("sandbox: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `CORTEX_HOME`, else `~/.cortex`.
fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("CORTEX_HOME") {
        return PathBuf::from(h);
    }
    let user_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    PathBuf::from(user_home).join(".cortex")
}

fn person() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "owner".into())
}

fn open_store() -> cortex::Result<Store> {
    let home = home();
    std::fs::create_dir_all(&home)?;
    Store::open(&home.join("cortex.db"))
}

/// The worker profile: the one named, or the default role's.
fn worker_profile(worker: Option<String>) -> cortex::Result<Profile> {
    let id = worker.unwrap_or_else(|| Roles::default().worker);
    find(&load_profiles(&home())?, &id)
}

fn engine(profile: Profile, fake_script: Option<PathBuf>) -> cortex::Result<Box<dyn Engine>> {
    match profile.engine {
        EngineKind::Fake => {
            let path = fake_script.ok_or_else(|| {
                cortex::Error::Invalid("the fake profile needs --fake-script".into())
            })?;
            Ok(Box::new(FakeEngine::load(&path)?))
        }
        EngineKind::ClaudeCli => Ok(Box::new(ClaudeEngine::new(
            PathBuf::from("claude"),
            profile,
        )?)),
        EngineKind::Api => Ok(Box::new(ApiEngine::with_profile(
            HttpTransport::from_env()?,
            profile,
        ))),
    }
}

fn run(
    repo: &std::path::Path,
    checks: &std::path::Path,
    worker: Option<String>,
    fake_script: Option<PathBuf>,
    request: &str,
) -> cortex::Result<ExitCode> {
    let mut engine = engine(worker_profile(worker)?, fake_script)?;
    let store = open_store()?;
    let m = mission::start(&store, &home(), repo, checks, request, &person())?;
    work(&store, &m, engine.as_mut())
}

/// Drives the mission and reports how it ended.
fn work(store: &Store, m: &MissionId, engine: &mut dyn Engine) -> cortex::Result<ExitCode> {
    println!("mission {}", m.0);
    std::io::stdout().flush()?;
    let outcome = mission::drive(store, &home(), m, engine, &Budgets::default())?;
    report(store, m, &outcome)?;
    Ok(match outcome {
        Outcome::Stuck { .. } => ExitCode::from(2),
        _ => ExitCode::SUCCESS,
    })
}

fn bench(
    worker: Option<String>,
    only: Option<&str>,
    dir: &std::path::Path,
) -> cortex::Result<ExitCode> {
    let profile = worker_profile(worker)?;
    let store = open_store()?;
    let mut make = |p: &cortex::bench::Prompt| match profile.engine {
        EngineKind::Fake => engine(profile.clone(), Some(p.fake_solution())),
        _ => engine(profile.clone(), None),
    };
    let (report, path) =
        cortex::bench::run(dir, only, &home(), &store, &mut make, &dir.join("results"))?;
    println!("passed {} of {}", report.passed, report.total);
    println!("results: {}", path.display());
    Ok(ExitCode::SUCCESS)
}

fn resume(
    m: &MissionId,
    worker: Option<String>,
    fake_script: Option<PathBuf>,
) -> cortex::Result<ExitCode> {
    let mut engine = engine(worker_profile(worker)?, fake_script)?;
    let store = open_store()?;
    mission::resume(&store, m)?;
    work(&store, m, engine.as_mut())
}

fn status(m: &MissionId) -> cortex::Result<ExitCode> {
    let store = open_store()?;
    let events: Vec<Event> = store.events(m)?.into_iter().map(|(_, e)| e).collect();
    let Some(Event::MissionStarted { request, .. }) = events.first() else {
        return Err(cortex::Error::Invalid(format!("no mission {}", m.0)));
    };
    println!("mission {}", m.0);
    println!("request: {request}");
    let sessions = events
        .iter()
        .filter(|e| matches!(e, Event::SessionStarted { .. }))
        .count();
    println!("sessions: {sessions}");
    let outcome = events.iter().rev().find_map(|e| match e {
        Event::MissionDone { commit } => Some(Outcome::Done {
            commit: commit.clone(),
        }),
        Event::MissionStuck { reason } => Some(Outcome::Stuck {
            reason: reason.clone(),
        }),
        Event::MissionPaused => Some(Outcome::Paused),
        _ => None,
    });
    match outcome {
        Some(o) => report(&store, m, &o)?,
        None => println!("state: working"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Says how it ended and, when done, where the result is and how to merge
/// it. Cortex never merges into the person's branch itself.
fn report(store: &Store, m: &MissionId, outcome: &Outcome) -> cortex::Result<()> {
    match outcome {
        Outcome::Done { commit } => {
            let repo = store
                .events(m)?
                .into_iter()
                .find_map(|(_, e)| match e {
                    Event::MissionStarted { repo_path, .. } => Some(repo_path),
                    _ => None,
                })
                .unwrap_or_default();
            let branch = mission::result_branch(m);
            println!("state: done, every check passed");
            println!("result: branch {branch} (commit {commit})");
            println!("to look:  git -C {} log -p {branch}", repo.display());
            println!("to merge: git -C {} merge {branch}", repo.display());
        }
        Outcome::Stuck { reason } => println!("state: stuck: {reason}"),
        Outcome::Paused => println!("state: paused (cortex resume {} continues it)", m.0),
    }
    Ok(())
}
