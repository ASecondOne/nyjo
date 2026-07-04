pub mod backup;
pub mod control;
pub mod init;
pub mod install;
pub mod model;
pub mod parser;
pub mod server;

pub use backup::{
    BackupKind, BackupRecord, LocalRestoreReport, StudioBackupIdentity, StudioBackupSnapshot,
    create_local_project_backup, create_studio_tree_backup, latest_local_project_backup,
    list_local_project_backups, read_studio_tree_backup, restore_local_project_backup,
};
pub use control::{
    BridgeCommandEnvelope, BridgeCommandKind, BridgeCommandResult, BridgeCommandRunReport,
    BridgeCommandState, BridgePlacesSnapshot, BridgeSessionSnapshot, BridgeTargetSelector,
    NyjoControlClient, QueueBridgeCommandRequest, QueueBridgeCommandResponse,
    SelectBridgeSessionRequest, SelectBridgeSessionResponse, resolve_target_session_id,
};
pub use init::{InitReport, init_project};
pub use install::{InstallReport, install_plugin};
pub use model::{InstanceNode, NodeMetadata, TreeSummary, summarize_tree};
pub use parser::scan_project;
