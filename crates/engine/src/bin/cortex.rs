use std::collections::HashMap;
use std::env;
use std::fmt::Display;
use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use cortex_core::autonomy::AutonomyDecision;
use cortex_core::contamination::*;
use cortex_core::ledger::{LedgerEntry, LedgerEvent};
use cortex_core::provider::ProviderId;
use cortex_engine::bandit::{ArmStats, UcbScorer};
use cortex_engine::evidence_floor::{check_floor, FloorVerdict};
use cortex_engine::pipeline::{plan_route, PipelineConfig, PipelineResult, RoutingPlan};
use cortex_engine::store::CortexStore;
use cortex_engine::templates::{ExecutionStep, RouteTemplate};

#[derive(Parser, Debug)]
#[command(
    name = "cortex",
    version,
    about = "User-facing CLI for the Cortex routing engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Route(RouteArgs),
    Status,
    Ledger(LedgerArgs),
    Explain(ExplainArgs),
    Templates(TemplatesArgs),
    Receipt(ReceiptArgs),
}

#[derive(Args, Debug)]
struct RouteArgs {
    goal: String,
    #[arg(long = "files", value_name = "PATH", num_args = 1.., required = true)]
    files: Vec<String>,
    #[arg(long, default_value_t = 5)]
    dial: u8,
}

#[derive(Args, Debug)]
struct LedgerArgs {
    #[arg(long, default_value_t = 10)]
    count: usize,
}

#[derive(Args, Debug)]
struct ExplainArgs {
    goal: String,
    #[arg(long = "files", value_name = "PATH", num_args = 0..)]
    files: Vec<String>,
    #[arg(long, default_value_t = 5)]
    dial: u8,
}

#[derive(Args, Debug)]
struct TemplatesArgs {
    #[arg(long)]
    dial: Option<u8>,
}

#[derive(Args, Debug)]
struct ReceiptArgs {
    goal: String,
    #[arg(long = "files", value_name = "PATH", num_args = 1.., required = true)]
    files: Vec<String>,
    #[arg(long, default_value_t = 5)]
    dial: u8,
    #[arg(long = "evidence", value_name = "EVIDENCE", num_args = 0..)]
    evidence: Vec<String>,
}

struct Palette {
    color: bool,
}

impl Palette {
    fn detect() -> Self {
        let term_ok = env::var("TERM").map(|term| term != "dumb").unwrap_or(false);
        Self {
            color: std::io::stdout().is_terminal() && term_ok,
        }
    }

    fn paint(&self, text: impl AsRef<str>, code: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{}\x1b[0m", text.as_ref())
        } else {
            text.as_ref().to_owned()
        }
    }

    fn heading(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "1;36")
    }

    fn label(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "1")
    }

    fn good(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "32")
    }

    fn warn(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "33")
    }

    fn bad(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "31")
    }

    fn muted(&self, text: impl AsRef<str>) -> String {
        self.paint(text, "2")
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let palette = Palette::detect();

    match cli.command {
        Commands::Route(args) => cmd_route(&palette, args),
        Commands::Status => cmd_status(&palette),
        Commands::Ledger(args) => cmd_ledger(&palette, args),
        Commands::Explain(args) => cmd_explain(&palette, args),
        Commands::Templates(args) => cmd_templates(&palette, args),
        Commands::Receipt(args) => cmd_receipt(&palette, args),
    }
}

fn cmd_route(palette: &Palette, args: RouteArgs) -> Result<()> {
    let store = open_default_store()?;
    let context = routing_context(&store, args.dial)?;
    let file_refs: Vec<&str> = args.files.iter().map(String::as_str).collect();

    match plan_route(&args.goal, &file_refs, &context.config, &context.scorer) {
        PipelineResult::Planned(plan) => {
            println!("{}", palette.heading("Routing Plan"));
            print_route_summary(palette, &plan);
            println!("{} {}", palette.label("Goal:"), args.goal);
            println!("{} {}", palette.label("Files:"), args.files.join(", "));
            print_steps(palette, &plan.steps);
            Ok(())
        }
        PipelineResult::Blocked {
            reason,
            missing_evidence,
        } => {
            println!("{}", palette.heading("Routing Blocked"));
            println!("{} {}", palette.label("Reason:"), palette.bad(reason));
            if !missing_evidence.is_empty() {
                println!(
                    "{} {}",
                    palette.label("Missing:"),
                    missing_evidence.join(", ")
                );
            }
            Ok(())
        }
        PipelineResult::NoProviders => {
            println!("{}", palette.heading("Routing Unavailable"));
            println!(
                "{} {}",
                palette.label("Reason:"),
                palette.bad("no providers are available"),
            );
            Ok(())
        }
    }
}

fn cmd_status(palette: &Palette) -> Result<()> {
    let store = open_default_store()?;
    let event_count = store.event_count()?;
    let arm_stats = store.load_arm_stats()?;

    println!("{}", palette.heading("Cortex Status"));
    println!(
        "{} {}",
        palette.label("Store:"),
        default_store_path().display()
    );
    println!("{} {}", palette.label("Total events:"), event_count);

    if arm_stats.is_empty() {
        println!("{}", palette.muted("No arm stats recorded yet."));
        return Ok(());
    }

    println!();
    println!("{}", palette.heading("Provider Summary"));
    for (provider, stats) in aggregate_provider_stats(&arm_stats) {
        let win_rate = percent(stats.successes, stats.trials);
        let reward = if stats.trials == 0 {
            0.0
        } else {
            stats.total_reward / stats.trials as f64
        };
        println!(
            "- {}  trials={}  wins={}  win_rate={}  mean_reward={:.2}",
            provider, stats.trials, stats.successes, win_rate, reward
        );
    }

    Ok(())
}

fn cmd_ledger(palette: &Palette, args: LedgerArgs) -> Result<()> {
    let store = open_default_store()?;
    let events = store.recent_events(args.count)?;

    println!("{}", palette.heading("Recent Events"));
    if events.is_empty() {
        println!("{}", palette.muted("Ledger is empty."));
        return Ok(());
    }

    for entry in events {
        print_ledger_entry(palette, &entry);
    }

    Ok(())
}

fn cmd_explain(palette: &Palette, args: ExplainArgs) -> Result<()> {
    let store = open_default_store()?;
    let context = routing_context(&store, args.dial)?;
    let file_refs: Vec<&str> = args.files.iter().map(String::as_str).collect();

    println!("{}", palette.heading("Routing Explanation"));
    println!("{} {}", palette.label("Goal:"), args.goal);
    println!("{} {}", palette.label("Dial:"), args.dial.clamp(1, 10));
    if args.files.is_empty() {
        println!(
            "{} {}",
            palette.label("Files:"),
            palette.muted("(none provided)")
        );
    } else {
        println!("{} {}", palette.label("Files:"), args.files.join(", "));
    }
    println!(
        "{} {}",
        palette.label("First observation:"),
        context.config.is_first_observation
    );
    println!(
        "{} {}",
        palette.label("Available providers:"),
        display_join(&context.config.available_providers)
    );
    println!(
        "{} {:.2}",
        palette.label("Exploration weight:"),
        context.scorer.exploration_weight
    );

    let available_templates = RouteTemplate::available_for_dial(args.dial.clamp(1, 10));
    println!();
    println!("{}", palette.heading("Template Candidates"));
    for template in &available_templates {
        let cfg = template.config();
        let eligible = (context.config.available_providers.len() as u8) >= cfg.min_providers;
        let marker = if eligible {
            palette.good("eligible")
        } else {
            palette.warn("needs more providers")
        };
        println!(
            "- {:?} [{}] providers>={} cost_x={:.1}  {}",
            template, marker, cfg.min_providers, cfg.estimated_cost_multiplier, cfg.description
        );
    }

    match plan_route(&args.goal, &file_refs, &context.config, &context.scorer) {
        PipelineResult::Planned(plan) => {
            println!();
            println!("{}", palette.heading("Decision"));
            print_route_summary(palette, &plan);

            println!();
            println!("{}", palette.heading("Provider Scores"));
            for provider in &context.config.available_providers {
                let score = provider_score_for_plan(&context, &plan, *provider);
                println!("- {} => {:.3}", provider, score);
            }

            println!();
            println!("{}", palette.heading("Autonomy Reasoning"));
            print_autonomy_reasoning(palette, &plan.autonomy, context.config.is_first_observation);

            println!();
            println!("{}", palette.heading("Evidence Floor"));
            match check_floor(plan.risk_level, &[]) {
                FloorVerdict::Satisfied { signals_met } => {
                    if signals_met.is_empty() {
                        println!(
                            "{}",
                            palette.good("No additional evidence is required at this risk level.")
                        );
                    } else {
                        println!("{} {}", palette.good("Satisfied:"), signals_met.join(", "));
                    }
                }
                FloorVerdict::Blocked { missing, .. } => {
                    println!(
                        "{} {}",
                        palette.warn("Required before execution is fully trusted:"),
                        missing.join(", ")
                    );
                }
            }

            println!();
            println!("{}", palette.heading("Execution Steps"));
            print_steps(palette, &plan.steps);
            Ok(())
        }
        PipelineResult::Blocked {
            reason,
            missing_evidence,
        } => {
            println!();
            println!("{}", palette.heading("Decision"));
            println!("{} {}", palette.label("Result:"), palette.bad("blocked"));
            println!("{} {}", palette.label("Reason:"), reason);
            if !missing_evidence.is_empty() {
                println!(
                    "{} {}",
                    palette.label("Missing evidence:"),
                    missing_evidence.join(", ")
                );
            }
            Ok(())
        }
        PipelineResult::NoProviders => {
            println!();
            println!("{}", palette.heading("Decision"));
            println!(
                "{} {}",
                palette.label("Result:"),
                palette.bad("no providers")
            );
            Ok(())
        }
    }
}

fn cmd_templates(palette: &Palette, args: TemplatesArgs) -> Result<()> {
    let templates = match args.dial {
        Some(dial) => RouteTemplate::available_for_dial(dial.clamp(1, 10)),
        None => RouteTemplate::all(),
    };

    println!("{}", palette.heading("Route Templates"));
    if let Some(dial) = args.dial {
        println!("{} {}", palette.label("Dial filter:"), dial.clamp(1, 10));
    }

    for template in templates {
        let cfg = template.config();
        println!(
            "- {:?}  dial={}..{}  providers>={}  cost_x={:.1}\n  {}",
            template,
            cfg.min_dial,
            cfg.max_dial,
            cfg.min_providers,
            cfg.estimated_cost_multiplier,
            cfg.description
        );
    }

    Ok(())
}

struct RoutingContext {
    config: PipelineConfig,
    scorer: UcbScorer,
}

fn routing_context(store: &CortexStore, dial: u8) -> Result<RoutingContext> {
    let arm_stats = store.load_arm_stats()?;
    let total_trials = arm_stats.values().map(|stats| stats.trials).sum();
    let scorer = UcbScorer {
        arms: arm_stats,
        exploration_weight: UcbScorer::exploration_weight_for_dial(dial),
        total_trials,
    };

    let config = PipelineConfig {
        dial: dial.clamp(1, 10),
        available_providers: vec![ProviderId::Claude, ProviderId::Openai],
        is_first_observation: store.event_count()? == 0,
    };

    Ok(RoutingContext { config, scorer })
}

fn open_default_store() -> Result<CortexStore> {
    let path = default_store_path();
    CortexStore::open(&path).with_context(|| format!("failed to open store at {}", path.display()))
}

fn default_store_path() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cortex").join("store.db")
}

fn aggregate_provider_stats(
    arm_stats: &std::collections::HashMap<cortex_engine::bandit::ArmKey, ArmStats>,
) -> Vec<(ProviderId, ArmStats)> {
    let mut aggregated: HashMap<ProviderId, ArmStats> = HashMap::new();

    for (key, stats) in arm_stats {
        let entry = aggregated.entry(key.provider).or_insert_with(|| ArmStats {
            successes: 0,
            trials: 0,
            total_reward: 0.0,
            last_updated: stats.last_updated,
        });
        entry.successes += stats.successes;
        entry.trials += stats.trials;
        entry.total_reward += stats.total_reward;
        if stats.last_updated > entry.last_updated {
            entry.last_updated = stats.last_updated;
        }
    }

    let mut rows: Vec<_> = aggregated.into_iter().collect();
    rows.sort_by_key(|(provider, _)| provider.to_string());
    rows
}

fn print_route_summary(palette: &Palette, plan: &RoutingPlan) {
    println!("{} {:?}", palette.label("Intent:"), plan.intent);
    println!("{} {:?}", palette.label("Risk:"), plan.risk_level);
    println!("{} {:?}", palette.label("Template:"), plan.template);
    println!("{} {}", palette.label("Provider:"), plan.provider);
    println!("{} {:?}", palette.label("Autonomy:"), plan.autonomy);
    println!("{} {:.2}", palette.label("Confidence:"), plan.confidence);
    println!("{} {}", palette.label("Explanation:"), plan.explanation);
}

fn print_steps(palette: &Palette, steps: &[ExecutionStep]) {
    println!();
    println!("{}", palette.heading("Steps"));
    for (index, step) in steps.iter().enumerate() {
        let provider = step
            .provider
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unassigned".to_string());
        println!(
            "{}. {:?} via {}  {}",
            index + 1,
            step.role,
            provider,
            step.description
        );
    }
}

fn print_ledger_entry(palette: &Palette, entry: &LedgerEntry) {
    let ts = entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC");
    match &entry.event {
        LedgerEvent::RoutingDecision {
            task_id,
            provider,
            tier,
            risk,
            score,
            model,
            ..
        } => {
            println!(
                "- [{}] routing_decision task={} provider={} tier={} risk={:?} score={:.2} model={}",
                ts,
                task_id,
                provider,
                tier,
                risk,
                score,
                model.as_deref().unwrap_or("-")
            );
        }
        LedgerEvent::TaskOutcome {
            task_id,
            provider,
            status,
            duration_ms,
            files_changed,
            tests_passed,
        } => {
            println!(
                "- [{}] task_outcome task={} provider={} status={:?} duration_ms={} files_changed={} tests_passed={}",
                ts,
                task_id,
                provider,
                status,
                duration_ms,
                files_changed,
                tests_passed
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
        }
        LedgerEvent::UserOverride {
            task_id,
            from_provider,
            to_provider,
            reason,
        } => {
            println!(
                "- [{}] user_override task={} {} -> {} reason={}",
                ts,
                task_id,
                from_provider,
                to_provider,
                reason.as_deref().unwrap_or("-")
            );
        }
        LedgerEvent::ProviderStatus {
            provider,
            authenticated,
            pressure,
        } => {
            let auth = if *authenticated {
                palette.good("ok")
            } else {
                palette.bad("no")
            };
            println!(
                "- [{}] provider_status provider={} authenticated={} pressure={:.2}",
                ts, provider, auth, pressure
            );
        }
    }
}

fn provider_score_for_plan(
    context: &RoutingContext,
    plan: &RoutingPlan,
    provider: ProviderId,
) -> f64 {
    let key = cortex_engine::bandit::ArmKey {
        task_family: cortex_engine::bandit::TaskFamily::from_intent(&plan.intent),
        risk_level: plan.risk_level,
        provider,
    };
    context.scorer.score(&key)
}

fn print_autonomy_reasoning(
    palette: &Palette,
    autonomy: &AutonomyDecision,
    is_first_observation: bool,
) {
    if is_first_observation {
        println!(
            "{}",
            palette.warn(
                "Store has no previous events, so autonomy is forced to ask before proceeding."
            )
        );
        return;
    }

    println!("{}", autonomy.description());
    if autonomy.requires_user_input() {
        println!(
            "{}",
            palette.warn("This route still requires a user checkpoint.")
        );
    } else {
        println!(
            "{}",
            palette.good("This route can proceed without a hard stop.")
        );
    }
}

fn display_join<T>(items: &[T]) -> String
where
    T: Display,
{
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn cmd_receipt(palette: &Palette, args: ReceiptArgs) -> Result<()> {
    let store = open_default_store()?;
    let context = routing_context(&store, args.dial)?;
    let file_refs: Vec<&str> = args.files.iter().map(String::as_str).collect();

    let signals = parse_evidence_args(&args.evidence);

    println!("{}", palette.heading("Evidence Receipt"));
    println!("{}", palette.muted("─".repeat(60)));

    match plan_route(&args.goal, &file_refs, &context.config, &context.scorer) {
        PipelineResult::Planned(plan) => {
            println!("{} {:?}", palette.label("Intent:"), plan.intent);
            println!("{} {:?}", palette.label("Risk:"), plan.risk_level);
            println!("{} {:?}", palette.label("Template:"), plan.template);
            println!("{} {}", palette.label("Provider:"), plan.provider);
            println!("{} {:.2}", palette.label("Confidence:"), plan.confidence);

            println!();
            println!("{}", palette.heading("Evidence Signals"));
            if signals.is_empty() {
                println!(
                    "{}",
                    palette.muted("  No evidence provided. Use --evidence to add signals.")
                );
                println!(
                    "{}",
                    palette.muted(
                        "  Formats: compiler, human-review, ai-test:model, ai-review:gen:ver"
                    )
                );
            } else {
                for (i, signal) in signals.iter().enumerate() {
                    let contamination_color = if signal.contamination.raw_value < 0.3 {
                        palette.good(format!("{:.2}", signal.contamination.raw_value))
                    } else if signal.contamination.raw_value < 0.7 {
                        palette.warn(format!("{:.2}", signal.contamination.raw_value))
                    } else {
                        palette.bad(format!("{:.2}", signal.contamination.raw_value))
                    };

                    let penalty_tag = if signal.contamination.same_model_penalty {
                        palette.bad(" [SAME-MODEL PENALTY]")
                    } else {
                        String::new()
                    };

                    println!(
                        "  {}. {} {:?} | tier={:?} | contamination={}{} | effective_reward={:.2}",
                        i + 1,
                        palette.label(&signal.description),
                        signal.source,
                        signal.tier,
                        contamination_color,
                        penalty_tag,
                        signal.effective_reward
                    );
                }
            }

            println!();
            println!("{}", palette.heading("Floor Verdict"));
            match check_floor(plan.risk_level, &signals) {
                FloorVerdict::Satisfied { signals_met } => {
                    if signals_met.is_empty() {
                        println!(
                            "  {}",
                            palette.good("PASS — no evidence required at this risk level")
                        );
                    } else {
                        println!("  {}", palette.good("PASS — all requirements satisfied:"));
                        for met in &signals_met {
                            println!("    {} {}", palette.good("*"), met);
                        }
                    }
                }
                FloorVerdict::Blocked {
                    missing,
                    risk_level,
                } => {
                    println!("  {} for {:?} risk:", palette.bad("BLOCKED"), risk_level,);
                    for m in &missing {
                        println!("    {} {}", palette.bad("x"), m);
                    }
                }
            }

            println!();
            println!("{}", palette.heading("Autonomy"));
            println!("  {} {:?}", palette.label("Decision:"), plan.autonomy);
            if plan.autonomy.requires_user_input() {
                println!(
                    "  {}",
                    palette.warn("User checkpoint required before execution.")
                );
            } else {
                println!(
                    "  {}",
                    palette.good("Can proceed without user intervention.")
                );
            }

            println!();
            println!("{}", palette.muted("─".repeat(60)));

            // Contamination summary
            if !signals.is_empty() {
                let total_raw: f64 = signals.iter().map(|s| s.raw_reward).sum();
                let total_effective: f64 = signals.iter().map(|s| s.effective_reward).sum();
                let avg_contamination: f64 = signals
                    .iter()
                    .map(|s| s.contamination.raw_value)
                    .sum::<f64>()
                    / signals.len() as f64;
                let penalty_count = signals
                    .iter()
                    .filter(|s| s.contamination.same_model_penalty)
                    .count();

                println!("{}", palette.heading("Contamination Summary"));
                println!(
                    "  {} {:.2}",
                    palette.label("Avg contamination:"),
                    avg_contamination
                );
                println!(
                    "  {} {:.2} -> {:.2} ({:.0}% retained)",
                    palette.label("Reward:"),
                    total_raw,
                    total_effective,
                    if total_raw > 0.0 {
                        total_effective / total_raw * 100.0
                    } else {
                        0.0
                    }
                );
                if penalty_count > 0 {
                    println!(
                        "  {}",
                        palette.bad(format!(
                            "{penalty_count} signal(s) penalized for same-model contamination"
                        ))
                    );
                }
            }
        }
        PipelineResult::Blocked {
            reason,
            missing_evidence,
        } => {
            println!("{} {}", palette.label("Result:"), palette.bad("BLOCKED"));
            println!("{} {}", palette.label("Reason:"), reason);
            if !missing_evidence.is_empty() {
                println!(
                    "{} {}",
                    palette.label("Missing:"),
                    missing_evidence.join(", ")
                );
            }
        }
        PipelineResult::NoProviders => {
            println!(
                "{} {}",
                palette.label("Result:"),
                palette.bad("NO PROVIDERS")
            );
        }
    }

    Ok(())
}

fn parse_evidence_args(args: &[String]) -> Vec<EvidenceSignal> {
    args.iter()
        .filter_map(|arg| {
            let parts: Vec<&str> = arg.split(':').collect();
            match parts[0] {
                "compiler" => Some(EvidenceSignal::new(
                    SignalTier::HardObjective,
                    EvidenceSource::CompilerOutput,
                    None,
                    None,
                    1.0,
                    "Compiler output",
                )),
                "ci" => Some(EvidenceSignal::new(
                    SignalTier::HardObjective,
                    EvidenceSource::CiPipeline,
                    None,
                    None,
                    1.0,
                    "CI pipeline",
                )),
                "linter" => Some(EvidenceSignal::new(
                    SignalTier::HardObjective,
                    EvidenceSource::Linter,
                    None,
                    None,
                    1.0,
                    "Linter output",
                )),
                "tests" => Some(EvidenceSignal::new(
                    SignalTier::HardObjective,
                    EvidenceSource::ExistingTestSuite,
                    None,
                    None,
                    1.0,
                    "Existing test suite",
                )),
                "human-review" => Some(EvidenceSignal::new(
                    SignalTier::IndependentVerify,
                    EvidenceSource::HumanReview,
                    None,
                    None,
                    1.0,
                    "Human review",
                )),
                "human-test" => Some(EvidenceSignal::new(
                    SignalTier::HardObjective,
                    EvidenceSource::HumanWrittenTest,
                    None,
                    None,
                    1.0,
                    "Human-written test",
                )),
                "ai-test" => {
                    let model = parts.get(1).map(|s| s.to_string());
                    let verifier = model.clone();
                    Some(EvidenceSignal::new(
                        SignalTier::IndependentVerify,
                        EvidenceSource::AiGeneratedTest,
                        model,
                        verifier,
                        1.0,
                        format!(
                            "AI-generated test{}",
                            parts.get(1).map(|m| format!(" ({m})")).unwrap_or_default()
                        ),
                    ))
                }
                "ai-review" => {
                    let generator = parts.get(1).map(|s| s.to_string());
                    let verifier = parts
                        .get(2)
                        .map(|s| s.to_string())
                        .or_else(|| generator.clone());
                    Some(EvidenceSignal::new(
                        SignalTier::IndependentVerify,
                        EvidenceSource::AiReview,
                        generator,
                        verifier,
                        1.0,
                        format!(
                            "AI review{}",
                            parts.get(1).map(|m| format!(" ({m})")).unwrap_or_default()
                        ),
                    ))
                }
                _ => None,
            }
        })
        .collect()
}

fn percent(successes: u32, trials: u32) -> String {
    if trials == 0 {
        return "n/a".to_string();
    }
    format!("{:.1}%", successes as f64 / trials as f64 * 100.0)
}
