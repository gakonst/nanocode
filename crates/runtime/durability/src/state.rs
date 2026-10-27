use std::collections::HashSet;

use crate::{EncodedPayload, Result, SessionRecord, StateReader, StateStore, StoreRecord};
use nanocode_durability_core::{DurableCheckpoint, JournalState};

pub use nanocode_durability_core::{
    IdentifiedSteerReceipt, OperationState, OperationStatus, Operations, SteerState, StepState,
    StepStatus, Transition,
};

pub(crate) type RetainedCheckpoint = nanocode_durability_core::RetainedCheckpoint<SessionRecord>;

#[derive(Clone, Debug, Default)]
pub struct DurableState {
    journal: JournalState,
    session: Option<SessionRecord>,
}

impl DurableState {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.journal.revision()
    }

    #[must_use]
    pub fn operations(&self) -> Operations<'_> {
        self.journal.operations()
    }

    #[must_use]
    pub fn operation(&self, operation_id: &str) -> Option<&OperationState> {
        self.journal.operation(operation_id)
    }

    #[must_use]
    pub fn has_operations(&self) -> bool {
        self.journal.has_operations()
    }

    #[must_use]
    pub fn pending_operations(&self) -> Vec<(&str, &OperationState)> {
        self.journal.pending_operations()
    }

    pub(crate) fn first_pending_operation(&self) -> Option<(&str, &OperationState)> {
        self.journal.first_pending_operation()
    }

    pub(crate) fn first_pending_operation_where(
        &self,
        predicate: impl FnMut(&str) -> bool,
    ) -> Option<(&str, &OperationState)> {
        self.journal.first_pending_operation_where(predicate)
    }

    #[must_use]
    pub const fn session(&self) -> Option<&SessionRecord> {
        self.session.as_ref()
    }

    pub(crate) fn set_session(&mut self, session: Option<SessionRecord>) {
        self.session = session;
        if !self.has_operations()
            && let Some(checkpoint) = self.journal.latest_checkpoint()
        {
            pin_start(self.session.as_mut(), checkpoint);
        }
    }

    pub(crate) const fn session_mut(&mut self) -> Option<&mut SessionRecord> {
        self.session.as_mut()
    }

    #[must_use]
    pub fn latest_checkpoint(&self) -> Option<&EncodedPayload> {
        self.journal.latest_checkpoint()
    }

    pub(crate) fn stage_records(&mut self) -> Vec<StoreRecord> {
        self.journal.stage_records()
    }

    pub(crate) fn stage_records_unless(
        &mut self,
        known: &dyn Fn(&str) -> bool,
    ) -> Vec<StoreRecord> {
        self.journal.stage_records_unless(known)
    }

    pub(crate) fn checkpoint_payload(&self) -> Result<String> {
        self.journal.checkpoint_payload(self.session.as_ref())
    }

    pub(crate) const fn receipts_complete(&self) -> bool {
        self.journal.receipts_loaded()
            && matches!(&self.session, Some(session) if !session.history_pruned)
    }

    pub(crate) fn seal_receipts(&mut self) -> Result<()> {
        self.journal.seal_receipts()
    }

    pub(crate) async fn hydrate(
        &mut self,
        store: &mut dyn StateStore,
        state_id: &str,
        read: &mut HashSet<String>,
    ) -> Result<()> {
        let mut pages = self.journal.receipt_pages();
        while !pages.is_empty() {
            let loaded = EncodedPayload::load_many_noting(&pages, store, state_id, read).await?;
            pages = self.journal.absorb_receipt_pages(loaded)?;
        }
        Ok(())
    }

    pub(crate) async fn hydrate_read(
        &mut self,
        reader: &mut dyn StateReader,
        state_id: &str,
    ) -> Result<()> {
        let mut pages = self.journal.receipt_pages();
        while !pages.is_empty() {
            let loaded = EncodedPayload::read_many(&pages, reader, state_id).await?;
            pages = self.journal.absorb_receipt_pages(loaded)?;
        }
        Ok(())
    }

    pub(crate) fn retain_terminal_receipts(&mut self, limit: usize) -> bool {
        let before = self.operations().len();
        let changed = self.journal.retain_terminal_receipts(limit);
        if self.operations().len() != before
            && let Some(session) = &mut self.session
        {
            session.history_pruned = true;
        }
        changed
    }

    pub(crate) fn from_checkpoint(revision: u64, checkpoint: DurableCheckpoint) -> Result<Self> {
        Ok(Self {
            journal: JournalState::from_checkpoint(revision, checkpoint)?,
            session: None,
        })
    }

    pub(crate) fn apply_transition(&mut self, revision: u64, entry: Transition) -> Result<()> {
        let pin = !self.has_operations() && matches!(entry, Transition::CheckpointCommitted { .. });
        self.journal.apply_transition(revision, entry)?;
        if pin && let Some(checkpoint) = self.journal.latest_checkpoint() {
            pin_start(self.session.as_mut(), checkpoint);
        }
        Ok(())
    }

    pub(crate) fn apply_within(&mut self, revision: u64, entry: Transition) -> Result<()> {
        let pin = !self.has_operations() && matches!(entry, Transition::CheckpointCommitted { .. });
        self.journal.apply_within(revision, entry)?;
        if pin && let Some(checkpoint) = self.journal.latest_checkpoint() {
            pin_start(self.session.as_mut(), checkpoint);
        }
        Ok(())
    }

    pub(crate) fn retracted(revision: u64) -> Self {
        Self {
            journal: JournalState::retracted(revision),
            session: None,
        }
    }

    pub(crate) fn advance_revision(&mut self, revision: u64) -> Result<()> {
        self.journal.advance_revision(revision)
    }
}

fn pin_start(session: Option<&mut SessionRecord>, checkpoint: &EncodedPayload) {
    if let Some(record) = session
        && record.branch.is_none()
        && matches!(
            record.lineage.origin,
            nanocode_agent_core::Origin::Fork | nanocode_agent_core::Origin::SideConversation
        )
    {
        record.start_checkpoint = Some(checkpoint.key().to_owned());
    }
}
