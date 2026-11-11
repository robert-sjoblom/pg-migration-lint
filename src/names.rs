//! Relation names as the catalog and the rules see them, after normalization.
//!
//! The parser produces [`QualifiedName`](crate::parser::ir::QualifiedName)s,
//! whose schema is optional. Normalization resolves each one against the
//! configured default schema into a [`ResolvedName`], so every name past that
//! point carries a real schema.

/// A schema-qualified relation: table, index, sequence, view.
///
/// Named after Postgres `pg_class` relations, which share one namespace per
/// schema. This is the catalog's key type. There is deliberately no `Display`
/// impl: user-facing text comes from [`ResolvedName::display_name`] or
/// `TableState::display_name`, so formatting a key can't leak a synthetic
/// default schema into a message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelationName {
    pub schema: String,
    pub name: String,
}

impl RelationName {
    /// Build a relation name from its schema and its name within that schema.
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }
}

/// A relation name from the SQL, resolved against the default schema.
///
/// Remembers whether the schema was written in the SQL or filled in by
/// normalization, so messages can show the name the way the user wrote it.
/// Equality and hashing compare the [`RelationName`] only. No `Display` impl:
/// messages call [`display_name`](Self::display_name).
#[derive(Debug, Clone, Eq)]
pub struct ResolvedName {
    relation: RelationName,
    schema_is_default: bool,
}

impl PartialEq for ResolvedName {
    fn eq(&self, other: &Self) -> bool {
        self.relation == other.relation
    }
}

impl std::hash::Hash for ResolvedName {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.relation.hash(state);
    }
}

impl ResolvedName {
    pub(crate) fn new(relation: RelationName, schema_is_default: bool) -> Self {
        Self {
            relation,
            schema_is_default,
        }
    }

    /// The catalog key this name resolves to.
    pub fn relation(&self) -> &RelationName {
        &self.relation
    }

    /// The bare relation name, without schema.
    pub fn name(&self) -> &str {
        &self.relation.name
    }

    /// The user-facing name: the bare name when the schema came from
    /// normalization, `schema.name` when the user wrote the schema.
    pub fn display_name(&self) -> String {
        if self.schema_is_default {
            self.relation.name.clone()
        } else {
            format!("{}.{}", self.relation.schema, self.relation.name)
        }
    }
}
