//! Deterministic Brain, provider and tool fixtures.
//!
//! Provider scripts are literal, ordered frame lists: a test that needs a
//! provider to disconnect mid-stream writes exactly that, rather than hoping a
//! real provider misbehaves.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::TestClock;
use crate::ids::IdFactory;
use crate::prefix::{PrefixError, RunPrefix};

/// The six admitted `BYOK` providers, in catalog order.
pub const PROVIDERS: [&str; 6] = [
    "anthropic",
    "deepseek",
    "google",
    "moonshotai",
    "openai",
    "zai",
];

/// One activation as the Brain journal would hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationFixture {
    /// Activation identifier.
    pub id: Uuid,
    /// Session the activation belongs to.
    pub session_id: Uuid,
    /// Exact generation the activation is fenced to.
    pub generation: u64,
    /// Start instant read from the injected clock.
    pub started_at: OffsetDateTime,
}

/// One journal entry and the effect identity it records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntryFixture {
    /// Entry identifier.
    pub id: Uuid,
    /// Owning activation.
    pub activation_id: Uuid,
    /// Position in the activation's journal.
    pub sequence: u64,
    /// Effect identity derived from the fold state.
    pub effect_id: Uuid,
    /// Record instant read from the injected clock.
    pub recorded_at: OffsetDateTime,
}

/// A scripted provider response, frame by frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderScriptFixture {
    /// Script identifier.
    pub id: Uuid,
    /// Provider this script impersonates.
    pub provider: String,
    /// Model slug within that provider.
    pub model: String,
    /// Ordered frames the script emits.
    pub frames: Vec<String>,
    /// Whether the script ends by disconnecting instead of completing.
    pub ends_ambiguously: bool,
}

/// Builds Brain fixtures from one clock, one seed and one run prefix.
#[derive(Debug, Clone)]
pub struct BrainFixtures {
    clock: TestClock,
    ids: IdFactory,
    prefix: RunPrefix,
    issued: u64,
}

impl BrainFixtures {
    /// A fixture factory bound to `prefix` and `seed`.
    #[must_use]
    pub fn new(prefix: RunPrefix, seed: u64) -> Self {
        Self {
            clock: TestClock::new(),
            ids: IdFactory::new(seed),
            prefix,
            issued: 0,
        }
    }

    /// The injected clock, so a test can advance it between fixtures.
    #[must_use]
    pub const fn clock(&self) -> &TestClock {
        &self.clock
    }

    /// Mutable access to the injected clock.
    pub const fn clock_mut(&mut self) -> &mut TestClock {
        &mut self.clock
    }

    /// The run prefix every synthetic name is built from.
    #[must_use]
    pub const fn prefix(&self) -> &RunPrefix {
        &self.prefix
    }

    /// A fresh activation for `session_id` at `generation`.
    #[must_use]
    pub fn activation(&mut self, session_id: Uuid, generation: u64) -> ActivationFixture {
        ActivationFixture {
            id: self.ids.next_id(),
            session_id,
            generation,
            started_at: self.clock.tick(),
        }
    }

    /// The next journal entry for `activation_id`.
    #[must_use]
    pub fn journal_entry(&mut self, activation_id: Uuid) -> JournalEntryFixture {
        let sequence = self.issued.saturating_add(1);
        self.issued = sequence;
        JournalEntryFixture {
            id: self.ids.next_id(),
            activation_id,
            sequence,
            effect_id: self.ids.next_id(),
            recorded_at: self.clock.tick(),
        }
    }

    /// A scripted provider response with `frame_count` frames.
    ///
    /// The provider is chosen from [`PROVIDERS`] in order, so a suite that asks
    /// for six scripts covers every admitted provider exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when the run prefix rejects the generated model
    /// slug.
    pub fn provider_script(
        &mut self,
        frame_count: usize,
        ends_ambiguously: bool,
    ) -> Result<ProviderScriptFixture, PrefixError> {
        let ordinal = self.issued;
        self.issued = self.issued.saturating_add(1);
        let index = usize::try_from(ordinal).unwrap_or(0) % PROVIDERS.len();
        let provider = PROVIDERS.get(index).copied().unwrap_or("anthropic");
        let model = self.prefix.resource(&format!("model-{ordinal:04}"))?;
        let frames = (0..frame_count)
            .map(|frame| format!("frame-{ordinal:04}-{frame:04}"))
            .collect();
        Ok(ProviderScriptFixture {
            id: self.ids.next_id(),
            provider: provider.to_owned(),
            model,
            frames,
            ends_ambiguously,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BrainFixtures, PROVIDERS};
    use crate::prefix::RunPrefix;
    use uuid::Uuid;

    fn factory() -> BrainFixtures {
        let prefix = RunPrefix::deterministic("brain", "0001").expect("a valid label");
        BrainFixtures::new(prefix, 5)
    }

    #[test]
    fn the_same_seed_and_prefix_produce_identical_fixtures() {
        let session = Uuid::from_u128(9);
        let mut left = factory();
        let mut right = factory();
        for generation in 0..8 {
            let activation = left.activation(session, generation);
            assert_eq!(activation, right.activation(session, generation));
            assert_eq!(
                left.journal_entry(activation.id),
                right.journal_entry(activation.id)
            );
            assert_eq!(
                left.provider_script(3, false)
                    .expect("a fixture name is accepted"),
                right
                    .provider_script(3, false)
                    .expect("a fixture name is accepted")
            );
        }
    }

    #[test]
    fn journal_sequences_are_strictly_increasing() {
        let mut fixtures = factory();
        let activation = fixtures.activation(Uuid::from_u128(1), 1);
        let mut previous = 0;
        for _ in 0..32 {
            let entry = fixtures.journal_entry(activation.id);
            assert!(entry.sequence > previous);
            assert_ne!(entry.id, entry.effect_id);
            previous = entry.sequence;
        }
    }

    #[test]
    fn six_scripts_cover_every_admitted_provider_once() {
        let mut fixtures = factory();
        let mut providers = Vec::new();
        for _ in 0..PROVIDERS.len() {
            let script = fixtures
                .provider_script(1, false)
                .expect("a fixture name is accepted");
            providers.push(script.provider);
        }
        providers.sort_unstable();
        assert_eq!(providers, PROVIDERS.to_vec());
    }

    #[test]
    fn scripts_carry_exactly_the_requested_frames() {
        let mut fixtures = factory();
        let script = fixtures
            .provider_script(4, true)
            .expect("a fixture name is accepted");
        assert_eq!(script.frames.len(), 4);
        assert!(script.ends_ambiguously);
        assert!(RunPrefix::is_synthetic(&script.model), "{}", script.model);
    }
}
