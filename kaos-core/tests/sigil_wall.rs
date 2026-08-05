//! Drawn sigils: the wall they live on, and the program a stack of them asks.
//!
//! The mandala is a rendering of a program — the picture *is* the code. A drawn
//! sigil is the opposite: compressed intent, which a model reads and answers
//! with a program. These tests cover the whole path from strokes to source,
//! without a window.

use kaos_core::ink::{letter_skeleton, Retention, Sigil, Stack, Stroke, Wall};

fn wall() -> Wall {
    let dir = std::env::temp_dir().join(format!(
        "kaos-wall-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Wall::new(dir)
}

fn mark() -> Sigil {
    let mut curve = Stroke::new(4.0);
    for step in 0..50 {
        let t = step as f32 / 49.0;
        let angle = t * std::f32::consts::TAU;
        curve.add(60.0 + angle.cos() * 40.0, 60.0 + (angle * 2.0).sin() * 30.0);
    }
    let mut slash = Stroke::new(3.0);
    slash.add(20.0, 100.0);
    slash.add(100.0, 20.0);
    Sigil {
        strokes: vec![curve, slash],
    }
}

#[test]
fn a_drawing_survives_being_put_away_and_taken_out_again() {
    let wall = wall();
    let drawn = mark();
    wall.save("intent", &drawn, 256).expect("save");

    let back = wall.load("intent").expect("load");
    assert_eq!(back.strokes.len(), drawn.strokes.len());
    // Points are kept to two decimals, which is finer than a pointer resolves.
    for (a, b) in back.strokes.iter().zip(&drawn.strokes) {
        assert_eq!(a.points.len(), b.points.len());
        for (p, q) in a.points.iter().zip(&b.points) {
            assert!((p.0 - q.0).abs() < 0.01 && (p.1 - q.1).abs() < 0.01);
        }
    }
    let _ = std::fs::remove_dir_all(wall.root());
}

/// Saving keeps two files: the strokes, so it can be drawn on again, and the
/// picture, because that is what a model is actually shown.
#[test]
fn saving_leaves_both_a_drawing_and_a_picture() {
    let wall = wall();
    let picture = wall.save("intent", &mark(), 128).expect("save");
    let (strokes, expected) = wall.paths("intent").expect("paths");
    assert_eq!(picture, expected);
    assert!(strokes.exists(), "the strokes are kept");
    assert!(picture.exists(), "and so is the image");
    let bytes = std::fs::read(&picture).expect("read");
    assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
    let _ = std::fs::remove_dir_all(wall.root());
}

/// Flat means flat. A drawing imports nothing, so it gets a name rather than a
/// path — and a name is data, so it must not be able to climb out.
#[test]
fn a_name_cannot_escape_the_wall() {
    let wall = wall();
    for hostile in [
        "../escape",
        "../../.ssh/id_rsa",
        "folder/inside",
        "",
        "   ",
        "with space",
        "dot.dot",
    ] {
        assert!(
            wall.paths(hostile).is_err(),
            "{hostile:?} should not be a usable name"
        );
    }
    for fine in ["intent", "the-queue", "attempt_2"] {
        assert!(wall.paths(fine).is_ok(), "{fine:?} should be fine");
    }
}

#[test]
fn the_wall_lists_what_is_on_it_and_can_forget() {
    let wall = wall();
    assert!(wall.names().is_empty());
    wall.save("second", &mark(), 64).expect("save");
    wall.save("first", &mark(), 64).expect("save");
    assert_eq!(
        wall.names(),
        vec!["first".to_string(), "second".to_string()]
    );

    wall.forget("first").expect("forget");
    assert_eq!(wall.names(), vec!["second".to_string()]);
    let (strokes, picture) = wall.paths("first").expect("paths");
    assert!(!strokes.exists() && !picture.exists(), "both files go");
    let _ = std::fs::remove_dir_all(wall.root());
}

#[test]
fn a_file_that_is_not_ours_is_refused_rather_than_guessed_at() {
    let wall = wall();
    std::fs::create_dir_all(wall.root()).expect("dir");
    let (strokes, _) = wall.paths("bogus").expect("paths");
    std::fs::write(&strokes, "not a sigil at all\n").expect("write");
    let refused = wall.load("bogus").expect_err("should refuse");
    assert!(
        refused.to_string().contains("kaos-sigil"),
        "the refusal should say what it wanted: {refused}"
    );
    let _ = std::fs::remove_dir_all(wall.root());
}

// ── the stack, and the program it asks for ────────────────────────────────

/// The whole point of stacking: intent is composed before anything is spent.
#[test]
fn a_stack_of_drawings_becomes_a_program_that_parses() {
    let wall = wall();
    let mut stack = Stack::default();
    for name in ["one", "two", "three"] {
        stack.push(wall.save(name, &mark(), 128).expect("save"));
    }
    let source = stack.program();

    // It is Rebis, and the language agrees.
    let parsed = rebis_lang::parse(&source).expect("the generated program must parse");
    assert_eq!(
        rebis_lang::parse(&rebis_lang::format(&parsed)).expect("reparse"),
        parsed
    );

    // Every drawing is in scope, and the ask is a `><`.
    for name in ["one", "two", "three"] {
        assert!(
            source.contains(name),
            "{name} should be framed in: {source}"
        );
    }
    assert_eq!(
        source.matches("(+ (&: ").count(),
        3,
        "one framing per sigil"
    );
    assert!(source.contains("(>< "), "the ask is a meta form");
    let _ = std::fs::remove_dir_all(wall.root());
}

#[test]
fn one_drawing_and_several_are_described_differently() {
    let mut one = Stack::default();
    one.push("a.png".into());
    assert!(one.instruction().contains("The attached image is a sigil"));

    let mut many = Stack::default();
    many.push("a.png".into());
    many.push("b.png".into());
    assert!(many.instruction().contains("2 attached images"));
    assert!(
        many.instruction().contains("in the order given"),
        "order is part of what was drawn"
    );
}

/// A sigil that needed a sentence to explain it is a sentence — but the option
/// exists, and when used it must reach the model.
#[test]
fn anything_said_alongside_the_marks_is_carried() {
    let mut stack = Stack::default();
    stack.push("a.png".into());
    stack.said = "the retry queue, after the fix".to_string();
    assert!(stack
        .instruction()
        .contains("the retry queue, after the fix"));
    assert!(rebis_lang::parse(&stack.program()).is_ok());
}

/// Quotes and backslashes in a path or a sentence must not end the prompt
/// early — the program is assembled, so anything unescaped is a syntax error
/// at best and a different program at worst.
#[test]
fn a_hostile_path_or_sentence_cannot_break_out_of_the_program() {
    let mut stack = Stack::default();
    stack.push(r#"/tmp/od"d\path.png"#.into());
    stack.said = "he said \"run it\"\nand left".to_string();
    let source = stack.program();
    rebis_lang::parse(&source).expect("still one well-formed program");
}

#[test]
fn an_empty_stack_asks_nothing() {
    let stack = Stack::default();
    assert!(stack.is_empty());
    // Still a valid program — a bare `><` with no sigils framed.
    assert!(rebis_lang::parse(&stack.program()).is_ok());
}

// ── Carroll's operation: construct, lose, charge ────────────────────────────

/// *Liber Null*, "Sigils": write the desire, strike out repeated letters, and
/// rearrange what survives into a glyph. Figure 2a.
#[test]
fn the_word_method_keeps_each_letter_once_in_the_order_it_appeared() {
    assert_eq!(
        letter_skeleton("I wish to obtain the Necronomicon"),
        "IWSHTOBANECRM"
    );
    assert_eq!(letter_skeleton("aabbcc"), "ABC");
    // Case is one letter, not two.
    assert_eq!(letter_skeleton("Aa Bb"), "AB");
    // Spaces, punctuation and digits are not part of the desire.
    assert_eq!(letter_skeleton("go! now, 42 — go"), "GONW");
    assert_eq!(letter_skeleton(""), "");
    assert_eq!(letter_skeleton("!!! 123 ..."), "");
    // Every surviving letter appears exactly once.
    let skeleton = letter_skeleton("the quick brown fox jumps over the lazy dog");
    assert_eq!(
        skeleton.len(),
        26,
        "a pangram survives as the whole alphabet"
    );
    for letter in skeleton.chars() {
        assert_eq!(skeleton.matches(letter).count(), 1, "{letter} twice");
    }
}

/// The plan quotes Carroll's worked example as `INSHTOBANECRM`; the method
/// applied to that sentence gives `IWSHTOBANECRM`, with the **W** of "I wish"
/// where the transcription has an **N**.
///
/// Pinned so nobody "fixes" the function back to the typo. The method is
/// specified precisely; a transcription is not, and the method wins.
#[test]
fn the_worked_example_differs_from_the_plan_by_one_letter() {
    let ours = letter_skeleton("I wish to obtain the Necronomicon");
    let quoted = "INSHTOBANECRM";
    assert_eq!(ours.len(), quoted.len(), "same thirteen letters");
    let differing: Vec<_> = ours
        .chars()
        .zip(quoted.chars())
        .filter(|(ours, quoted)| ours != quoted)
        .collect();
    assert_eq!(differing, vec![('W', 'N')]);
}

/// The scaffold is drawn over and discarded. It is not a stroke, so it cannot
/// reach the raster a model is shown — a model reading the letters would be
/// reading the desire, which is the opposite of a sigil.
#[test]
fn the_scaffold_is_not_part_of_the_drawing() {
    let sigil = mark();
    let before = sigil.raster(64);
    // `letter_skeleton` is a pure function over text. There is no path from it
    // into `Sigil`, and this is the assertion that keeps it that way: the type
    // has one field and it holds strokes.
    let _scaffold = letter_skeleton("I wish to obtain the Necronomicon");
    assert_eq!(sigil.raster(64), before);
}

/// *"To successfully lose the sigil, both the sigil form and the associated
/// desire must be banished from normal waking consciousness."*
///
/// The acceptance criterion as written: commit a distinctive desire under
/// `Lose`, then grep the whole wall directory for it.
#[test]
fn a_lost_desire_appears_in_no_file_on_the_wall() {
    let wall = wall();
    let desire = "PHLOGISTON-QUINTESSENCE-9317";

    wall.commit("lost", &mark(), desire, Retention::Lose, 64)
        .expect("commits");

    let mut searched = 0;
    for entry in std::fs::read_dir(wall.root())
        .expect("the wall exists")
        .flatten()
    {
        let bytes = std::fs::read(entry.path()).expect("readable");
        searched += 1;
        assert!(
            !contains(&bytes, desire.as_bytes()),
            "{} still holds the desire",
            entry.path().display()
        );
    }
    assert!(searched >= 2, "the glyph and its strokes were written");
    assert_eq!(wall.desire("lost"), None);
    // And the glyph itself survives — losing the desire is not losing the work.
    assert!(wall.load("lost").is_ok());
    assert!(wall.names().contains(&"lost".to_string()));
}

#[test]
fn keeping_is_the_default_and_is_what_a_person_gets_unless_they_choose() {
    let wall = wall();
    assert_eq!(Retention::default(), Retention::Keep);
    assert!(Retention::default().keeps_the_desire());

    wall.commit("kept", &mark(), "the retry queue", Retention::Keep, 64)
        .expect("commits");
    assert_eq!(wall.desire("kept").as_deref(), Some("the retry queue"));
}

/// Re-committing a kept sigil under `Lose` must not leave the old sentence
/// beside the new glyph — that is the one outcome the mode exists to prevent.
#[test]
fn losing_a_sigil_that_was_kept_removes_what_was_kept() {
    let wall = wall();
    wall.commit("mind", &mark(), "SALAMANDER-4471", Retention::Keep, 64)
        .expect("commits");
    assert!(wall.desire("mind").is_some());

    wall.commit("mind", &mark(), "SALAMANDER-4471", Retention::Lose, 64)
        .expect("commits again");
    assert_eq!(wall.desire("mind"), None);
    for entry in std::fs::read_dir(wall.root()).expect("the wall").flatten() {
        let bytes = std::fs::read(entry.path()).expect("readable");
        assert!(!contains(&bytes, b"SALAMANDER-4471"), "{:?}", entry.path());
    }
}

#[test]
fn forgetting_a_sigil_forgets_its_desire_too() {
    let wall = wall();
    wall.commit("gone", &mark(), "UNDINE-8823", Retention::Keep, 64)
        .expect("commits");
    wall.forget("gone").expect("forgets");
    assert_eq!(wall.desire("gone"), None);
    assert!(!wall.names().contains(&"gone".to_string()));
}

/// B3. *"A record should be kept of all work with sigils but not in such a way
/// as to cause conscious deliberation over the sigilized desire."*
///
/// A run's record echoes the program it ran, so the test that matters is about
/// the program: under `Lose` the sentence never enters it. Not redacted
/// afterwards — never written, so there is no copy to miss.
#[test]
fn a_lost_stack_keeps_the_glyph_in_the_program_and_not_the_desire() {
    let mut stack = Stack::default();
    stack.push("/tmp/glyph-one.png".into());
    stack.said = "BASILISK-2205".to_string();
    stack.retention = Retention::Lose;

    let source = stack.program();
    rebis_lang::parse(&source).expect("still one well-formed program");
    assert!(
        !source.contains("BASILISK-2205"),
        "the desire reached the program: {source}"
    );
    assert!(
        !stack.instruction().contains("BASILISK-2205"),
        "the desire reached the prompt"
    );
    // What the record DOES keep: the glyph, so the work is traceable.
    assert!(source.contains("glyph-one.png"), "{source}");
    assert!(!stack.keeps_the_desire());
}

#[test]
fn a_kept_stack_still_carries_what_was_said() {
    let mut stack = Stack::default();
    stack.push("/tmp/glyph.png".into());
    stack.said = "the retry queue".to_string();
    assert_eq!(stack.retention, Retention::Keep);
    assert!(stack.program().contains("the retry queue"));
}

#[test]
fn clearing_a_stack_returns_it_to_keeping() {
    // A `Lose` chosen for one working is not a setting that follows the next
    // one. The mode is per-operation, and forgetting that would silently lose a
    // desire somebody meant to keep.
    let mut stack = Stack {
        retention: Retention::Lose,
        said: "something".to_string(),
        ..Default::default()
    };
    stack.clear();
    assert_eq!(stack.retention, Retention::Keep);
}

/// Substring search over bytes — the wall holds PNGs as well as text.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
