//! Backend-neutral conformance harness for `TextWriter`, `Committable`, and `TextDeleter`.
//!
//! Available when the `conformance` feature is enabled.

use std::fmt::Debug;

use crate::store::{Committable, TextDeleter, TextWriter};

/// Verify that a backend implementation fulfills the expected trait contracts.
///
/// Backends should call this in their own test suites with a fresh, empty
/// store instance.
pub async fn verify_backend<S, Opts, Id, Err>(store: &S, default_options: Opts) -> Result<(), Err>
where
    S: TextWriter<Options = Opts, Id = Id, Error = Err>
        + Committable<Error = Err>
        + TextDeleter<Id = Id, Error = Err>,
    Opts: Clone,
    Id: Clone + PartialEq + Debug,
    Err: core::error::Error,
{
    // Write a single entry
    let id_1 = store
        .write_text("conformance record 1", default_options.clone())
        .await?;

    // Write a second entry
    let id_2 = store
        .write_text("conformance record 2", default_options.clone())
        .await?;

    assert_ne!(id_1, id_2, "backend must assign unique IDs");

    // Commit writes
    store.commit().await?;

    // Delete one entry
    store.delete_text(id_1).await?;

    // Commit delete
    store.commit().await?;

    Ok(())
}
