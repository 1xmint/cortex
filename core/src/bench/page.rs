//! The results page: one self-contained HTML file per run, with no
//! scripts. Model-written text is untrusted, so every piece of report text
//! goes through `escape`.

use super::Report;
use super::score::{Attempt, PerAccepted};
use std::fmt::Write;

const STYLE: &str = "
:root { --bg: #ffffff; --fg: #1b1f24; --muted: #59636e; --line: #d1d9e0;
  --ok: #1a7f37; --bad: #cf222e; --card: #f6f8fa; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #0d1117; --fg: #e6edf3; --muted: #9198a1; --line: #3d444d;
    --ok: #3fb950; --bad: #f85149; --card: #151b23; }
}
body { background: var(--bg); color: var(--fg); margin: 0 auto; max-width: 60rem;
  padding: 1rem; font: 15px/1.5 system-ui, sans-serif; }
dl.head { display: grid; grid-template-columns: max-content 1fr; gap: .2rem 1rem; }
dt { color: var(--muted); }
dd { margin: 0; }
section { background: var(--card); border: 1px solid var(--line);
  border-radius: 6px; padding: .5rem 1rem; margin: 1rem 0; }
h2 { font-size: 1.05rem; margin: .3rem 0; }
.ok { color: var(--ok); } .bad { color: var(--bad); } .muted { color: var(--muted); }
pre { white-space: pre-wrap; overflow-wrap: anywhere; margin: .3rem 0;
  border-left: 3px solid var(--line); padding-left: .5rem; }
code { overflow-wrap: anywhere; }
";

/// The one place report text is made safe for HTML.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn usd(micros: u64) -> String {
    format!("${}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

fn per_accepted(p: &PerAccepted) -> String {
    match p {
        PerAccepted::Usd { micros } => usd(*micros),
        PerAccepted::Unbounded => "unbounded (nothing accepted)".into(),
        PerAccepted::Unknown => "unknown".into(),
    }
}

fn estimated(micros: Option<u64>) -> String {
    micros.map_or_else(|| "unknown".into(), usd)
}

pub fn render(report: &Report) -> String {
    let s = &report.summary;
    let total_cost = report
        .attempts
        .iter()
        .map(|a| a.estimated_usd_micros)
        .sum::<Option<u64>>();
    let title = format!("Bench: {} on {}", report.arm, report.worker_profile);
    let mut h = String::new();
    let _ = write!(
        h,
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>{STYLE}</style></head><body>\n<h1>{}</h1>\n<dl class=\"head\">",
        escape(&title),
        escape(&title)
    );
    let rows = [
        ("Arm", report.arm.clone()),
        ("Worker", report.worker_profile.clone()),
        (
            "Engine",
            format!("{} {}", report.engine, report.engine_version),
        ),
        (
            "Provisional",
            if report.provisional {
                "yes (price or limits not confirmed)".into()
            } else {
                "no".into()
            },
        ),
        ("Tier", report.tier.clone()),
        (
            "Pass rate",
            format!(
                "{} of {} accepted ({:.0}%)",
                s.accepted,
                s.total,
                s.pass_rate * 100.0
            ),
        ),
        ("Time", format!("{:.0} s", s.seconds)),
        ("Tokens", s.tokens.to_string()),
        ("Estimated cost", estimated(total_cost)),
        (
            "Cost per accepted result",
            per_accepted(&s.cost_per_accepted),
        ),
        ("Started", report.started.clone()),
        ("Cortex commit", report.cortex_commit.clone()),
    ];
    for (k, v) in rows {
        let _ = write!(h, "<dt>{}</dt><dd>{}</dd>", escape(k), escape(&v));
    }
    h.push_str("</dl>\n");
    for a in &report.attempts {
        story(&mut h, a);
    }
    h.push_str("</body></html>\n");
    h
}

fn story(h: &mut String, a: &Attempt) {
    let (class, word) = if a.accepted {
        ("ok", "accepted")
    } else {
        ("bad", "not accepted")
    };
    let _ = write!(
        h,
        "<section><h2>{} <span class=\"muted\">try {}</span> <span class=\"{class}\">{word}</span></h2>",
        escape(&a.task),
        a.try_no
    );
    let st = &a.story;
    let _ = write!(
        h,
        "<p class=\"muted\">{} sessions, {:.0} s, {} tokens, estimated {}</p>",
        a.sessions,
        a.seconds,
        a.tokens,
        escape(&estimated(a.estimated_usd_micros))
    );
    let _ = write!(h, "<p><b>Request</b></p><pre>{}</pre>", escape(&st.request));
    let files = if st.files_changed.is_empty() {
        "none".to_string()
    } else {
        st.files_changed
            .iter()
            .map(|f| format!("<code>{}</code>", escape(f)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = write!(h, "<p><b>Files changed:</b> {files}</p>");
    let commit = st.commit.as_deref().map_or("none".into(), escape);
    let _ = write!(h, "<p><b>Commit:</b> <code>{commit}</code></p>");
    for f in &st.failures {
        let _ = write!(
            h,
            "<p class=\"bad\">Failed: <code>{}</code>{}</p><pre>{}</pre>",
            escape(&f.check_id),
            if f.hidden { " (hidden)" } else { "" },
            escape(&f.output_tail)
        );
    }
    if let Some(why) = &st.stuck_reason {
        let _ = write!(h, "<p><b>Stuck:</b> {}</p>", escape(why));
    }
    h.push_str("</section>\n");
}
