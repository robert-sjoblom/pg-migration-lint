//! SQL parsing and IR generation

pub mod ir;
pub(crate) mod pg_query;

pub use ir::{
    AlterTable, AlterTableAction, Cluster, ColumnDef, CreateIndex, CreateTable, DefaultExpr,
    DeleteFrom, DropIndex, DropSchema, DropTable, IndexColumn, InsertInto, IrNode, Located,
    PartitionBy, PartitionStrategy, QualifiedName, SourceSpan, TableConstraint, TablePersistence,
    TriggerDisableScope, TruncateTable, TypeName, UpdateTable,
};
