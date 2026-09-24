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
            // Invariant: a scheme with no backend here cannot say the referent is missing.
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

/// The product `done` requires when a schema or a contract needs it; resolving to `Ok(None)`,
/// or a declared schema with no resolver to read it, refuses rather than skips (section 6.3).
pub(super) fn check_output(
    resolve: Option<&dyn OutputResolve>,
    plan: &Plan,
    label: &TodoLabel,
    output: Option<&Url>,
) -> Result<Option<String>, PlanOpError> {
    let todo = plan.todo(label);
    let declared = todo
        .and_then(|todo| todo.delegation.as_ref())
        .and_then(|delegation| delegation.output.as_ref());
    let contracted = todo.is_some_and(|todo| todo.contract.is_some());
    if let (Some(declared), None) = (declared, output) {
        return Err(PlanOpError::MissingDeclaredOutput {
            label: label.clone(),
            schema: declared.schema.clone(),
        });
    }
    if declared.is_none() && !contracted {
        return Ok(None);
    }
    let Some(url) = output else {
        return Ok(None);
    };
    let Some(resolve) = resolve else {
        return match declared {
            Some(_) => Err(PlanOpError::UnservedOutput {
                label: label.clone(),
                url: url.clone(),
            }),
            None => Ok(None),
        };
    };
    let product = resolve
        .resolve(url)
        .map_err(|cause| PlanOpError::UnresolvedOutput {
            label: label.clone(),
            url: url.clone(),
            cause,
        })?
        .ok_or_else(|| PlanOpError::UnservedOutput {
            label: label.clone(),
            url: url.clone(),
        })?;
    if let Some(declared) = declared {
        let schema = &declared.schema;
        let document = resolve
            .resolve(schema)
            .map_err(|cause| PlanOpError::UnusableSchema {
                label: label.clone(),
                schema: Box::new(schema.clone()),
                cause,
            })?
            .ok_or_else(|| PlanOpError::UnservedSchema {
                label: label.clone(),
                schema: Box::new(schema.clone()),
            })?;
        validate_product(label, url, schema, &product, &document)?;
    }
    Ok(Some(product))
}
