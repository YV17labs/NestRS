//! The Markdown a measurement prints: one row per run, then the median and the
//! spread (min–max) of every column.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

pub struct Table {
    title: String,
    columns: Vec<&'static str>,
    runs: Vec<Vec<f64>>,
    notes: Vec<String>,
}

impl Table {
    pub fn new(title: String, columns: &[&'static str]) -> Self {
        Self {
            title,
            columns: columns.to_vec(),
            runs: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn run(&mut self, values: Vec<f64>) {
        debug_assert_eq!(values.len(), self.columns.len());
        self.runs.push(values);
    }

    pub fn note(&mut self, note: String) {
        self.notes.push(note);
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "\n### {}\n\n| run | {} |\n|---|",
            self.title,
            self.columns.join(" | ")
        );
        out.push_str(&"---:|".repeat(self.columns.len()));
        out.push('\n');
        for (index, values) in self.runs.iter().enumerate() {
            row(
                &mut out,
                &(index + 1).to_string(),
                values.iter().map(|v| number(*v)),
            );
        }
        let column = |at: usize| -> Vec<f64> { self.runs.iter().map(|run| run[at]).collect() };
        let columns: Vec<Vec<f64>> = (0..self.columns.len()).map(column).collect();
        row(
            &mut out,
            "**median**",
            columns.iter().map(|c| number(percentile(c, 50.0))),
        );
        row(
            &mut out,
            "spread",
            columns.iter().map(|c| {
                let (low, high) = (percentile(c, 0.0), percentile(c, 100.0));
                format!("{}–{}", number(low), number(high))
            }),
        );
        for note in &self.notes {
            let _infallible = writeln!(out, "\n{note}");
        }
        out
    }
}

fn row(out: &mut String, label: &str, cells: impl Iterator<Item = String>) {
    let cells: Vec<String> = cells.collect();
    let _infallible = writeln!(out, "| {label} | {} |", cells.join(" | "));
}

/// The nearest-rank percentile of `values`; 0 and 100 are the extremes.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn number(value: f64) -> String {
    match value.abs() {
        v if v >= 100.0 || v == 0.0 => format!("{value:.0}"),
        v if v >= 10.0 => format!("{value:.1}"),
        _ => format!("{value:.2}"),
    }
}

pub fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The commands Redis ran between two readings, busiest first.
pub fn busiest(
    before: &BTreeMap<String, u64>,
    after: &BTreeMap<String, u64>,
) -> Vec<(String, u64)> {
    let mut delta: Vec<(String, u64)> = after
        .iter()
        .map(|(name, calls)| {
            let earlier = before.get(name).copied().unwrap_or(0);
            (name.clone(), calls.saturating_sub(earlier))
        })
        .filter(|(_, calls)| *calls > 0)
        .collect();
    delta.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    delta
}

/// `busiest` as one line, each command's calls divided by `per`.
pub fn breakdown(commands: &[(String, u64)], per: f64, unit: &str, top: usize) -> String {
    commands
        .iter()
        .take(top)
        .map(|(name, calls)| format!("`{name}` {}{unit}", number(*calls as f64 / per)))
        .collect::<Vec<_>>()
        .join(", ")
}
