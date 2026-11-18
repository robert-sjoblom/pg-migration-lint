//! The pre-existing partitioned parent a statement takes ACCESS EXCLUSIVE
//! on. PGM024 reports each statement that locks one, and PGM026 counts the
//! parents one transaction locks.

use std::collections::HashSet;

use crate::names::{RelationName, ResolvedName};
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::LintContext;

/// A statement that takes ACCESS EXCLUSIVE on a partitioned parent which
/// existed before the change.
pub(super) struct ParentLock<'s> {
    /// What the statement does to the partition.
    pub(super) statement: ParentLockingStatement,
    /// The partition the statement drops or creates.
    pub(super) child: &'s ResolvedName,
    /// The parent the statement locks.
    pub(super) parent: LockedParent,
}

/// The two statements that lock a partition's parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ParentLockingStatement {
    /// `DROP TABLE child`, where `child` is a partition.
    DropPartition,
    /// `CREATE TABLE child PARTITION OF parent`.
    CreatePartitionOf,
}

/// A partitioned parent a statement locks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LockedParent {
    /// The parent's catalog key.
    pub(super) key: RelationName,
    /// The parent's name for messages.
    pub(super) display_name: String,
}

/// Finds the parent locks of one unit's statements.
pub(super) struct ParentLocks<'s> {
    /// Children the unit detaches. A unit can only detach a child before
    /// dropping it (a `DETACH` after the `DROP` fails), so the child is no
    /// partition when it is dropped and the drop locks no parent: the safe
    /// pattern PGM004 recommends.
    detached_in_unit: HashSet<&'s RelationName>,
}

impl<'s> ParentLocks<'s> {
    /// Prepare to find parent locks among `statements`, the statements of
    /// one unit.
    pub(super) fn of_unit(statements: &'s [Located<IrNode>]) -> Self {
        let detached_in_unit = statements
            .iter()
            .filter_map(|stmt| match &stmt.node {
                IrNode::AlterTable(at) => Some(&at.actions),
                _ => None,
            })
            .flatten()
            .filter_map(|action| match action {
                AlterTableAction::DetachPartition { child, .. } => Some(child.relation()),
                _ => None,
            })
            .collect();
        Self { detached_in_unit }
    }

    /// The pre-existing parent `node` takes ACCESS EXCLUSIVE on, if any. A
    /// parent counts when it is in `catalog_before` and not created in the
    /// change.
    pub(super) fn taken_by(
        &self,
        node: &'s IrNode,
        ctx: &LintContext<'_>,
    ) -> Option<ParentLock<'s>> {
        match node {
            IrNode::DropTable(dt) => {
                let child_key = dt.name.relation();
                if self.detached_in_unit.contains(child_key) {
                    return None;
                }
                let parent_key = ctx
                    .catalog_before
                    .get_table(child_key)?
                    .parent_table
                    .clone()?;
                if !ctx.is_existing_table(&parent_key) {
                    return None;
                }
                let display_name = ctx.parent_display_name(child_key)?;
                Some(ParentLock {
                    statement: ParentLockingStatement::DropPartition,
                    child: &dt.name,
                    parent: LockedParent {
                        key: parent_key,
                        display_name,
                    },
                })
            }
            IrNode::CreateTable(ct) => {
                let parent_name = ct.partition_of.as_ref()?;
                if !ctx.is_existing_table(parent_name.relation()) {
                    return None;
                }
                Some(ParentLock {
                    statement: ParentLockingStatement::CreatePartitionOf,
                    child: &ct.name,
                    parent: LockedParent {
                        key: parent_name.relation().clone(),
                        display_name: parent_name.display_name(),
                    },
                })
            }
            _ => None,
        }
    }
}
