//! Safe Git workspace, checkpoint, diff, and source-control operations.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkingTreeCondition {
    Clean,
    Dirty,
    Conflicted,
}
