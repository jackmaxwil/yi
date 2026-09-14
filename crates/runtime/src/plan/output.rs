//! The declared-output check at `done`: the product resolves and satisfies its schema.

use yi_types::plan::doc::{Plan, TodoLabel};
use yi_types::url::Url;

use super::ops::PlanOpError;

/// One read over the fetch seam, shaped like [`crate::fetch::CheckpointShow`]. `Ok(None)` is
/// resolved-but-unserved: the referent exists, its bytes are not there to adjudicate.
pub trait OutputResolve: Send + Sync {
    fn resolve(&self, url: &Url) -> Result<Option<String>, String>;
}

impl OutputResolve for crate::fetch::Resolver {
    fn resolve(&self, url: &Url) -> Result<Option<String>, String> {
        use crate::fetch::FetchError;
        match self.fetch(url) {
            Ok(fetched) => Ok(Some(fetched.text)),
            // Invariant: a missing reader is not a missing referent: a scheme
            // this resolver has no backend for cannot adjudicate existence.
            Err(FetchError::Unsupported { .. }) => Ok(None),
            Err(
                error @ (FetchError::Denied { .. }
                | FetchError::External { .. }
                | FetchError::OutsideWorkspace { .. }
                | FetchError::BadAddress { .. }
                | FetchError::NotFound { .. }
                | FetchError::Stale { .. }
                | FetchError::Backend { .. }),
            ) => Err(error.to_string()),
        }
    }
}

/// The section 3 clause the existence probe alone leaves unenforced: a delegation that
/// declared a schema makes `done` legal only on a product that satisfies it.
fn validate_product(
    label: &TodoLabel,
    url: &Url,
    schema: &Url,
    product: &str,
    document: &str,
) -> Result<(), PlanOpError> {
    let unusable = |cause: String| PlanOpError::UnusableSchema {
        label: label.clone(),
        schema: Box::new(schema.clone()),
        cause,
    };
    let mismatch = |detail: String| PlanOpError::OutputMismatch {
        label: label.clone(),
        url: Box::new(url.clone()),
        schema: Box::new(schema.clone()),
        detail,
    };
    let document: serde_json::Value =
        serde_json::from_str(document).map_err(|error| unusable(error.to_string()))?;
    let value = crate::schema::extract(product).map_err(mismatch)?;
    crate::schema::Schema::from_value(document)
        .map_err(unusable)?
        .validate(&value)
        .map_err(mismatch)
}

/// `done` on a todo whose delegation declares an output schema: the output is required, and
/// with a resolver it must resolve and validate, else the refusal names which failed.
pub(super) fn check_output(
    resolve: Option<&dyn OutputResolve>,
    plan: &Plan,
    label: &TodoLabel,
    output: Option<&Url>,
) -> Result<(), PlanOpError> {
    let Some(declared) = plan
        .todo(label)
        .and_then(|todo| todo.delegation.as_ref())
        .and_then(|delegation| delegation.output.as_ref())
    else {
        return Ok(());
    };
    let Some(url) = output else {
        return Err(PlanOpError::MissingDeclaredOutput {
            label: label.clone(),
            schema: declared.schema.clone(),
        });
    };
    let Some(resolve) = resolve else {
        return Ok(());
    };
    let product = resolve
        .resolve(url)
        .map_err(|cause| PlanOpError::UnresolvedOutput {
            label: label.clone(),
            url: url.clone(),
            cause,
        })?;
    let schema = &declared.schema;
    let document = resolve
        .resolve(schema)
        .map_err(|cause| PlanOpError::UnusableSchema {
            label: label.clone(),
            schema: Box::new(schema.clone()),
            cause,
        })?;
    if let (Some(product), Some(document)) = (product, document) {
        validate_product(label, url, schema, &product, &document)?;
    }
    Ok(())
}
