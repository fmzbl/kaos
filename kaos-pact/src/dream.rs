//! Dreaming — the offline divination pass between banished workings.
//!
//! Liber Null, "Dreaming": the dream state is *egress into divination*. All
//! humans dream nightly but the brain censors it "to prevent it interfering
//! with waking consciousness" — exactly the tunnel discarding its rotting
//! middle, and exactly the spiral banishing a fizzled context. Yet that
//! discarded material is where the missed connection lives.
//!
//! Carroll gives two operational rules. (1) "The only method of gaining full
//! access is to keep a book ... record all dreams as soon as possible after
//! waking" — recording defeats the censor. (2) Control comes from *selecting
//! a topic*: "the dream is set up by strongly visualizing the desired topic
//! in an otherwise silenced mind, immediately before sleep."
//!
//! So the Dream, mechanically: when a working is banished, before the next
//! self is summoned, a TOOLLESS pass re-reads the failure — seeded by the
//! intent (the topic) and the crossed gnosis (the recorded dream-book) — at
//! lunar temperature (the divinatory, associative pole). It touches nothing;
//! its sole output is ONE hypothesis, a divination of what the waking
//! working missed, which seeds the next attempt's context. Dreams are
//! hypotheses, never actions: the gate always wakes before anything ships.
//!
//! This module is pure prompt-craft + distillation (testable offline). The
//! single model call lives at the call site, where the mind's `Spec` is in
//! scope — the dream is a divination through the same seam every working uses.

/// Build the (system, user) prompt for a dream pass. The system prompt puts
/// the mind in the divinatory pole — no tools, no fixing, one connection.
/// The user prompt is the topic (the task) plus the dream-book (the gnosis
/// crossed from the banished working).
pub fn dream_prompt(task: &str, gnosis: &str) -> (String, String) {
    let system = "You are dreaming. You hold NO tools and will change nothing — a dream is \
         divination, not action. A previous waking attempt at the task below was banished; \
         its record follows. Find the ONE connection the waking work missed: the wrong \
         assumption, the unread neighbour, the class of input never probed, the cause \
         upstream of the symptom. Answer in one or two sentences — a single hypothesis the \
         next attempt should test first. Do not restate the task; do not apologise; name \
         the missed thing."
        .to_string();
    let user = format!(
        "TASK (the topic of the dream):\n{task}\n\nThe banished working's record:\n{gnosis}"
    );
    (system, user)
}

/// Distill a dream reply into a seed hypothesis, or None if the dream was
/// empty/degenerate. Keeps it short — a dream seeds, it does not dictate.
pub fn distill(reply: &str) -> Option<String> {
    let t = reply.trim();
    if t.is_empty() {
        return None;
    }
    // A dream that just echoes "I cannot" or refuses carries no charge.
    let low = t.to_lowercase();
    if low.starts_with("i cannot") || low.starts_with("i can't") || low.starts_with("i don't") {
        return None;
    }
    // Bound it: the first ~600 chars, whole — the freshest divination, not an essay.
    let cut: String = t.chars().take(600).collect();
    Some(cut)
}

/// Weave a distilled hypothesis into the next attempt's intent, framed as a
/// dream (a lead to test, not a fact to trust — the gate still decides).
pub fn seed(hypothesis: &str) -> String {
    format!("\nA dream of the banished working suggests, to test FIRST (verify it — dreams mislead):\n{hypothesis}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_carries_topic_and_record() {
        let (sys, user) = dream_prompt(
            "fix the rounding bug",
            "edited currency.py; gate: AssertionError 0.12 != 0.13",
        );
        assert!(sys.contains("dreaming") && sys.contains("NO tools"));
        assert!(user.contains("fix the rounding bug"));
        assert!(user.contains("AssertionError"));
    }

    #[test]
    fn distill_rejects_empty_and_refusals_keeps_signal() {
        assert_eq!(distill("   "), None);
        assert_eq!(distill("I cannot determine the cause."), None);
        let h = distill(
            "The fix rounded with ROUND_HALF_EVEN; the parser at line 40 must round HALF_UP.",
        )
        .unwrap();
        assert!(h.contains("HALF_UP"));
        // long dreams are bounded
        let long = "x".repeat(2000);
        assert_eq!(distill(&long).unwrap().chars().count(), 600);
    }

    #[test]
    fn seed_frames_the_dream_as_a_lead_not_a_fact() {
        let s = seed("check the inverse-rate path");
        assert!(s.contains("test FIRST") && s.contains("dreams mislead"));
        assert!(s.contains("check the inverse-rate path"));
    }
}
