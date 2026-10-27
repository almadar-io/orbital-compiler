//! Whether an orbital is a program or an atom.

use orbital_core_base::schema::{Orbital, OrbitalSchema};

/// A program is an orbital with pages, or any orbital of a multi-orbital file; a lone orbital with no
/// pages is an atom, reached by events and specified with `expects page`.
pub(crate) fn is_program(schema: &OrbitalSchema, orbital: &Orbital) -> bool {
    orbital.pages.as_ref().is_some_and(|p| !p.is_empty())
        || schema.orbitals.iter().filter(|o| !o.is_spec()).count() > 1
}

