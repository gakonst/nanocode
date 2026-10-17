#![deny(rustdoc::broken_intra_doc_links)]

mod backend;
mod branch;
mod catalog;
mod child_journal;
mod format;
mod memory;
mod paged;
mod session;
mod shared_store;
mod state;

pub use backend::{Branches, DurableAgentExt, DurableBackendRegistration, agent_error};
pub use branch::{BranchTurn, CheckpointBranch};
pub use catalog::{BranchBoundary, InitialSettings, SessionRecord};
pub use catalog::{
    BranchPoint, IntoSessionStore, LIST_LIMIT, SessionPage, SessionQuery, SessionReader,
    SessionStore, SessionSummary, StoredSession, StoredTurn, TranscriptItem, TurnStatus,
};
pub use child_journal::{JournalClaim, JournalSeed};
pub use format::{AdmittedTurn, CheckpointFormat, FormatFuture};
pub use memory::MemoryStore;
pub use nanocode_durability_core::{
    Committed, EncodedPayload, Error, EventCursor, EventHit, EventHits, EventOrder, EventPage,
    EventQuery, EventRange, IndexedState, OwnedState, OwnerId, OwnerToken, Result, StateCommit,
    StateCursor, StatePage, StateQuery, StateReader, StateStore, StateSummary, StoreError,
    StoreEvent, StoreFuture, StoreRecord, StoreReplacement, StoreUpdate, StoredEvent, StoredState,
    SummaryUpdate,
};
pub use paged::{PagedWriter, PayloadReader, Prepared, Sequence, load_sequence};
pub use session::{
    Admission, AutomaticAdmission, BeginStep, DurableOwner, DurableSession, InitialAcquisition,
    MetadataExpectation, MetadataWrite, SessionReceipt, SessionWrite, StoredSteer, TitleChange,
};
pub use shared_store::SharedStore;
pub use state::{
    DurableState, IdentifiedSteerReceipt, OperationState, OperationStatus, SteerState, StepState,
    StepStatus, Transition,
};
