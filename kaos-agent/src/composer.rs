//! Rebis-native intent composition.
//!
//! Chaos mode is a Rebis program, not a host instruction appended to a chat.
//! The composer constructs one `($ prompt intent)` expression, runs it through
//! the ordinary Rebis oracle, and adopts only a response that parses as Rebis.
//! This keeps model output on the data side of the execution boundary: prose is
//! never executable, and a generated program is still only source until the
//! caller explicitly runs it.

use std::cell::RefCell;
use std::time::Duration;

use crate::backend::Sampling;
use crate::provider::Spec;

/// Compose one user intent into validated Rebis source.
pub fn compose_program(
    spec: &Spec,
    intent: &str,
    timeout: Duration,
    sampling: Sampling,
) -> Result<String, String> {
    let source = kaos_core::chaos::composition_source(intent);
    let expression = rebis_lang::parse(&source)
        .map_err(|error| format!("internal composer source is invalid: {error}"))?;
    let oracle = CompletionOracle {
        spec,
        timeout,
        sampling,
        error: RefCell::new(None),
    };
    let mut record = rebis_lang::Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate(&expression, &mut record, &oracle);
    if let Some(error) = oracle.error.into_inner() {
        return Err(error);
    }
    if let Some(diagnostic) = result.diagnostics.first() {
        return Err(format!("Rebis composer failed: {diagnostic}"));
    }
    let reply = result
        .output
        .or_else(|| {
            result
                .firings
                .into_iter()
                .rev()
                .find_map(|firing| firing.answer)
        })
        .filter(|reply| !reply.trim().is_empty())
        .ok_or_else(|| "Rebis composer returned no source".to_string())?;
    kaos_core::chaos::valid_program(&reply)
}

struct CompletionOracle<'a> {
    spec: &'a Spec,
    timeout: Duration,
    sampling: Sampling,
    error: RefCell<Option<String>>,
}

impl rebis_lang::Oracle for CompletionOracle<'_> {
    fn fire(&self, prompt: &str) -> Option<String> {
        self.try_fire(prompt).ok().flatten()
    }

    fn try_fire(&self, prompt: &str) -> Result<Option<String>, String> {
        match self
            .spec
            .complete_sampled("", prompt, self.timeout, Some(self.sampling))
        {
            Ok(reply) => Ok(Some(reply)),
            Err(error) => {
                *self.error.borrow_mut() = Some(error.clone());
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_composer_is_a_real_rebis_program_before_a_model_is_called() {
        let source = kaos_core::chaos::composition_source("inspect the parser");
        let expression = rebis_lang::parse(&source).expect("composer source should parse");
        assert!(matches!(expression, rebis_lang::Expr::Concat(_)));
        assert!(source.contains("($"));
        assert!(source.contains("inspect the parser"));
    }
}
