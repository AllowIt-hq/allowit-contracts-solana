//! Native policy source, copied byte-for-byte into both chain builds.
//! Amounts use six-decimal policy units. Only daily_limit is explicitly tunable.
use crate::policy_api::{enforce_daily_limit, require_approval};
use crate::policy_api::{Context, PolicyError};

pub fn execute(ctx: &Context) -> Result<u64, PolicyError> {
    require_approval(ctx)?;
    enforce_daily_limit(ctx)
}
