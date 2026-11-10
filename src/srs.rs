//! Per-concept mastery, and the rule for what to show next.
//!
//! The model is deliberately small and explainable: every concept carries a
//! strength in `0.0..=1.0` and a due time. A correct annotation stretches the
//! interval, a wrong or unknown one collapses it so the same syntax comes back
//! within the same week — the "Duolingo" half of the idea.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const DAY: f64 = 86_400.0;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Correct,
    Partial,
    Wrong,
    Unknown,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Correct => "correct",
            Verdict::Partial => "partial",
            Verdict::Wrong => "wrong",
            Verdict::Unknown => "unknown",
        }
    }

    /// Tolerant parse of whatever a reviewer wrote ("Right", "missed", ...).
    pub fn from_label(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "correct" | "right" | "ok" | "good" | "yes" => Verdict::Correct,
            "partial" | "partly" | "incomplete" | "half" => Verdict::Partial,
            "wrong" | "incorrect" | "missing" | "missed" | "no" => Verdict::Wrong,
            _ => Verdict::Unknown,
        }
    }

    /// Higher is worse — used to pick the block's overall grade.
    pub fn severity(self) -> u8 {
        match self {
            Verdict::Correct => 0,
            Verdict::Partial => 1,
            Verdict::Wrong => 2,
            Verdict::Unknown => 3,
        }
    }
}

/// Folds the verdicts a single concept collected across several blocks into one.
/// "I don't know" anywhere wins: not knowing it once means it needs work.
pub fn combine(verdicts: &[Verdict]) -> Verdict {
    if verdicts.is_empty() {
        return Verdict::Unknown;
    }
    if verdicts.contains(&Verdict::Unknown) {
        return Verdict::Unknown;
    }
    if verdicts.iter().all(|v| *v == Verdict::Correct) {
        return Verdict::Correct;
    }
    if verdicts
        .iter()
        .any(|v| matches!(v, Verdict::Correct | Verdict::Partial))
    {
        return Verdict::Partial;
    }
    Verdict::Wrong
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptState {
    pub id: String,
    /// `0.0` = never seen, `1.0` = solid.
    #[serde(default)]
    pub strength: f64,
    #[serde(default)]
    pub reps: u32,
    #[serde(default)]
    pub lapses: u32,
    #[serde(default)]
    pub interval_days: f64,
    /// Unix seconds; `0` means "due now" (never scheduled).
    #[serde(default)]
    pub due_at: u64,
}

impl ConceptState {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            strength: 0.0,
            reps: 0,
            lapses: 0,
            interval_days: 0.0,
            due_at: 0,
        }
    }

    pub fn apply(&mut self, verdict: Verdict, now: u64) {
        match verdict {
            Verdict::Correct => {
                self.strength = (self.strength + (1.0 - self.strength) * 0.40).min(1.0);
                self.interval_days = if self.interval_days <= 0.0 {
                    1.0
                } else {
                    (self.interval_days * 2.2).min(120.0)
                };
                self.due_at = now + (self.interval_days * DAY) as u64;
            }
            Verdict::Partial => {
                self.strength = (self.strength + (1.0 - self.strength) * 0.15).min(1.0);
                self.interval_days = if self.interval_days <= 0.0 {
                    0.5
                } else {
                    (self.interval_days * 1.3).min(60.0)
                };
                self.due_at = now + (self.interval_days * DAY) as u64;
            }
            Verdict::Wrong => {
                self.strength *= 0.45;
                self.interval_days = 0.25;
                self.lapses += 1;
                self.due_at = now + (0.25 * DAY) as u64;
            }
            Verdict::Unknown => {
                self.strength *= 0.75;
                self.interval_days = 0.0;
                self.lapses += 1;
                // Back inside the current sitting, not tomorrow.
                self.due_at = now + 900;
            }
        }
        self.reps += 1;
    }

    /// How much this concept wants to be seen right now. Overdue and weak wins.
    pub fn urgency(&self, now: u64) -> f64 {
        let gap = 1.0 - self.strength;
        let timing = if self.due_at == 0 {
            4.0
        } else if self.due_at <= now {
            1.0 + ((now - self.due_at) as f64 / DAY).min(3.0)
        } else {
            0.3 / (1.0 + (self.due_at - now) as f64 / DAY)
        };
        timing * (0.35 + gap)
    }

    pub fn is_mastered(&self) -> bool {
        self.strength >= 0.85 && self.reps >= 3
    }
}

/// A short human phrase for a due time, relative to `now`.
pub fn describe_due(now: u64, due_at: u64) -> String {
    if due_at == 0 {
        return "new".to_string();
    }
    if due_at <= now {
        let late = (now - due_at) as f64;
        return if late < 3600.0 {
            "due now".to_string()
        } else if late < DAY {
            format!("{:.0}h overdue", late / 3600.0)
        } else {
            format!("{:.0}d overdue", late / DAY)
        };
    }
    let left = (due_at - now) as f64;
    if left < 3600.0 {
        format!("in {:.0}m", left / 60.0)
    } else if left < DAY {
        format!("in {:.0}h", left / 3600.0)
    } else {
        format!("in {:.0}d", left / DAY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;

    #[test]
    fn correct_builds_strength_and_stretches_the_interval() {
        let mut card = ConceptState::new("rust.ownership.move");
        card.apply(Verdict::Correct, NOW);
        assert!((card.strength - 0.40).abs() < 1e-9);
        assert_eq!(card.interval_days, 1.0);
        assert_eq!(card.due_at, NOW + 86_400);

        card.apply(Verdict::Correct, NOW);
        assert!((card.strength - 0.64).abs() < 1e-9);
        assert!((card.interval_days - 2.2).abs() < 1e-9);
        assert_eq!(card.reps, 2);
        assert_eq!(card.lapses, 0);
    }

    #[test]
    fn unknown_collapses_the_interval_to_within_the_sitting() {
        let mut card = ConceptState::new("c");
        card.apply(Verdict::Correct, NOW);
        card.apply(Verdict::Unknown, NOW);
        assert_eq!(card.interval_days, 0.0);
        assert_eq!(card.due_at, NOW + 900);
        assert_eq!(card.lapses, 1);
        assert!((card.strength - 0.30).abs() < 1e-9);
    }

    #[test]
    fn wrong_punishes_harder_than_unknown() {
        let mut a = ConceptState::new("a");
        let mut b = ConceptState::new("b");
        a.apply(Verdict::Correct, NOW);
        b.apply(Verdict::Correct, NOW);
        a.apply(Verdict::Wrong, NOW);
        b.apply(Verdict::Unknown, NOW);
        assert!(a.strength < b.strength);
        assert!(a.due_at > b.due_at);
    }

    #[test]
    fn strength_never_leaves_its_bounds() {
        let mut card = ConceptState::new("c");
        for _ in 0..50 {
            card.apply(Verdict::Correct, NOW);
        }
        assert!(card.strength <= 1.0);
        assert!(card.interval_days <= 120.0);
        assert!(card.is_mastered());
    }

    #[test]
    fn interval_is_capped() {
        let mut card = ConceptState::new("c");
        for _ in 0..40 {
            card.apply(Verdict::Correct, NOW);
        }
        assert_eq!(card.interval_days, 120.0);
    }

    #[test]
    fn combine_never_hides_not_knowing() {
        assert_eq!(
            combine(&[Verdict::Correct, Verdict::Unknown]),
            Verdict::Unknown
        );
        assert_eq!(
            combine(&[Verdict::Correct, Verdict::Partial]),
            Verdict::Partial
        );
        assert_eq!(combine(&[Verdict::Wrong, Verdict::Wrong]), Verdict::Wrong);
        assert_eq!(
            combine(&[Verdict::Correct, Verdict::Correct]),
            Verdict::Correct
        );
        assert_eq!(combine(&[]), Verdict::Unknown);
    }

    #[test]
    fn unseen_concepts_are_the_most_urgent() {
        let unseen = ConceptState::new("new").urgency(NOW);
        let mut known = ConceptState::new("known");
        known.apply(Verdict::Correct, NOW);
        assert!(unseen > known.urgency(NOW));
    }

    #[test]
    fn labels_parse_tolerantly() {
        assert_eq!(Verdict::from_label(" Right "), Verdict::Correct);
        assert_eq!(Verdict::from_label("MISSED"), Verdict::Wrong);
        assert_eq!(Verdict::from_label("gibberish"), Verdict::Unknown);
    }

    #[test]
    fn due_descriptions_read_like_english() {
        assert_eq!(describe_due(NOW, 0), "new");
        assert_eq!(describe_due(NOW, NOW - 10), "due now");
        assert_eq!(describe_due(NOW, NOW + 2 * 86_400), "in 2d");
    }
}
