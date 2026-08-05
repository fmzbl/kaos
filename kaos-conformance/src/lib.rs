//! End-to-end conformance for the Rebis language.
//!
//! Every other test in this workspace scripts the oracle: an answer is decided
//! in advance and the test checks what the runtime did with it. That proves the
//! runtime and proves nothing about the language meeting a real model, which is
//! the only place it is ever used. These tests run whole programs against a
//! local model and check what actually came back.
//!
//! **What is asserted is structural, never textual.** A small model will not
//! reliably answer with exactly the word you asked for, so a test that demanded
//! one would fail for the wrong reason and teach nothing. What a model cannot
//! fake is the shape of the run: how many times it was called, which prompts it
//! saw, what reached each one as `INPUT:`, which branch was expanded, what was
//! marked to be kept. Those are facts about the language, and they are the ones
//! checked here.
//!
//! The model is [`MODEL`] — small, local, and free, because a conformance suite
//! that costs money per run is a suite nobody runs. The programs live beside
//! this file as `.rebis` sources rather than string literals: they are meant to
//! be read, edited, and run by hand with `kaos rebis run`, and a program that
//! only exists inside a test is a program nobody looks at.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rebis_lang::{
    Attachment, ExecutionEvent, ExecutionScope, Inlet, ModuleName, ModuleResolver, Oracle,
    Orchestration, Received, Record, RuntimeDiagnostic, RuntimeLimits,
};

/// The model every program is run against.
///
/// Small on purpose. A conformance suite proves the *language*, and a language
/// property that only holds on a frontier model is not a language property. It
/// also has to be free to run, or it stops being run.
pub const DEFAULT_MODEL: &str = "qwen3:4b";

/// The model this run actually uses.
///
/// `KAOS_CONFORMANCE_MODEL` overrides [`DEFAULT_MODEL`], so these tests take
/// the same knob `run.sh` takes and the two cannot drift into disagreeing
/// about what was measured. The shell suite defaults to a faster model for a
/// stated reason — a fifty-program pass on a reasoning model is an hour — while
/// these twenty-odd in-process tests stay on the 4B, where being slower is
/// affordable and being a reasoning model is occasionally the point.
#[must_use]
pub fn model() -> String {
    std::env::var("KAOS_CONFORMANCE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

/// How long one call may take. A 4B model on CPU is not fast, and a timeout
/// that fires is indistinguishable from a bug in the report.
pub const TIMEOUT: Duration = Duration::from_secs(300);

/// Where the `.rebis` programs live.
#[must_use]
pub fn programs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("programs")
}

/// One program's source, by file name.
///
/// # Panics
///
/// If the program is missing — a named program that is not there is a broken
/// suite, not a failing test.
#[must_use]
pub fn program(name: &str) -> String {
    let path = programs().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// A local model, recorded.
///
/// Every prompt is kept in order, which is what the assertions read: the shape
/// of a run is the sequence of prompts it produced, and that is a fact about
/// the language rather than about the model's prose.
pub struct Local {
    prompts: Mutex<Vec<String>>,
    attachments: Mutex<Vec<usize>>,
}

impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

impl Local {
    #[must_use]
    pub fn new() -> Self {
        Self {
            prompts: Mutex::new(Vec::new()),
            attachments: Mutex::new(Vec::new()),
        }
    }

    /// Every prompt this run produced, in order.
    #[must_use]
    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().expect("not poisoned").clone()
    }

    /// How many model calls the run made.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.prompts().len()
    }

    /// How many files each call carried.
    #[must_use]
    pub fn attachments(&self) -> Vec<usize> {
        self.attachments.lock().expect("not poisoned").clone()
    }
}

impl Oracle for Local {
    fn fire(&self, prompt: &str) -> Option<String> {
        self.try_fire_attached(prompt, &[], None, &ExecutionScope::root(), 0)
            .ok()
            .flatten()
    }

    fn try_fire_attached(
        &self,
        prompt: &str,
        attached: &[Attachment],
        _model: Option<&str>,
        _scope: &ExecutionScope,
        _call: usize,
    ) -> Result<Option<String>, String> {
        self.prompts
            .lock()
            .expect("not poisoned")
            .push(prompt.to_string());
        self.attachments
            .lock()
            .expect("not poisoned")
            .push(attached.len());
        let (answer, refusals) = kaos_agent::provider::Spec::parse(&format!("ollama:{}", model()))
            .complete_attached("", prompt, attached, TIMEOUT, None);
        for refusal in refusals {
            eprintln!("attach not sent · {refusal}");
        }
        // Trimming is all the normalisation there is. The point of these tests
        // is what the RUNTIME did, and rewriting a model's answer would hide
        // exactly the transport being tested.
        answer.map(|text: String| Some(text.trim().to_string()))
    }
}

/// Modules resolve from the embedded standard library only.
pub struct Std;

impl ModuleResolver for Std {
    fn resolve(&self, _module: &ModuleName) -> Result<Option<String>, String> {
        // `WithStd` wraps every resolver, so `std/*` is already served before a
        // host sees the name. Anything else is genuinely absent.
        Ok(None)
    }
}

/// A host that wires nothing and reads nothing — the default posture.
pub struct Bare;

impl Inlet for Bare {
    fn ask(&self, _label: Option<&str>) -> Option<Received> {
        None
    }
}

/// A host that reads files under the crate, and answers an ask with a fixed
/// line so `(&)` can be exercised without a person at a terminal.
pub struct Wired;

impl Inlet for Wired {
    fn ask(&self, _label: Option<&str>) -> Option<Received> {
        Some(Received {
            text: "The delivered value is: cinnabar.".to_string(),
            attachments: Vec::new(),
        })
    }

    fn load(&self, source: &str) -> Option<Received> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(source);
        let text = std::fs::read_to_string(path).ok()?;
        Some(Received {
            text,
            attachments: Vec::new(),
        })
    }
}

/// Run a program against the local model, with a host that wires nothing.
///
/// # Panics
///
/// If the program does not parse — a conformance program that cannot be read
/// is a broken suite.
pub fn run(name: &str) -> (Orchestration, Local) {
    run_with(name, &Bare)
}

/// Run a program against the local model and a host of your choosing.
///
/// # Panics
///
/// If the program does not parse.
pub fn run_with(name: &str, inlet: &dyn Inlet) -> (Orchestration, Local) {
    let source = program(name);
    let expression = rebis_lang::parse(&source)
        .unwrap_or_else(|error| panic!("{name} does not parse: {error}"));
    let oracle = Local::new();
    let mut record = Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate_with_inlet(
        &expression,
        &mut record,
        &oracle,
        &Std,
        &InletRef(inlet),
        // Generous: these are structure tests, and a budget that stops a
        // program mid-way would be reported as a language failure.
        RuntimeLimits::default(),
        &mut |_| {},
    );
    (result, oracle)
}

/// Run a program and keep the record it built.
///
/// [`run`] throws the record away, which is right for every assertion about
/// firings and prompts. It is exactly wrong for the imaginary space, whose
/// entire claim is about what memory holds afterwards: the same program with
/// and without braces fires the same prompts in the same order, so the trace
/// cannot tell them apart and only the record can.
///
/// # Panics
///
/// If the program does not parse.
pub fn run_recording(name: &str) -> (Orchestration, Local, Record) {
    let source = program(name);
    let expression = rebis_lang::parse(&source)
        .unwrap_or_else(|error| panic!("{name} does not parse: {error}"));
    let oracle = Local::new();
    let mut record = Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate_with_inlet(
        &expression,
        &mut record,
        &oracle,
        &Std,
        &InletRef(&Bare),
        RuntimeLimits::default(),
        &mut |_| {},
    );
    (result, oracle, record)
}

/// Whether the record can reach anything at all under a topic.
///
/// The question every imaginary-space assertion asks, in the form the language
/// itself asks it — a flashback's own resolution, not a substring search, so a
/// test cannot pass on a coincidence the runtime would never have recalled.
#[must_use]
pub fn remembers(record: &Record, topic: &str) -> bool {
    !record
        .recall(topic, rebis_lang::FLASHBACK_LINES, rebis_lang::FLASHBACK_CHARS)
        .is_empty()
}

/// A trait object needs a concrete wrapper to satisfy the generic entry point.
struct InletRef<'a>(&'a dyn Inlet);

impl Inlet for InletRef<'_> {
    fn ask(&self, label: Option<&str>) -> Option<Received> {
        self.0.ask(label)
    }
    fn load(&self, source: &str) -> Option<Received> {
        self.0.load(source)
    }
}

/// Every diagnostic a run reported, as strings — for a failure message that
/// says what went wrong rather than only that something did.
#[must_use]
pub fn diagnostics(result: &Orchestration) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Assert a run reported nothing, showing what it reported when it did.
///
/// # Panics
///
/// When the run has any diagnostic.
pub fn assert_clean(name: &str, result: &Orchestration) {
    assert!(
        result.diagnostics.is_empty(),
        "{name} reported {:?}",
        diagnostics(result)
    );
}

/// Whether a run reported an unavailable input — the expected state of a
/// program that obtains something under a host that wires nothing.
#[must_use]
pub fn unavailable(result: &Orchestration) -> bool {
    result
        .diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic, RuntimeDiagnostic::InputUnavailable { .. }))
}

/// The branch decisions a run made, in order.
#[must_use]
pub fn decisions(result: &Orchestration) -> Vec<bool> {
    result
        .events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::BranchSelected { decision } => Some(*decision),
            _ => None,
        })
        .collect()
}
