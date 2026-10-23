use utoipa::{
    openapi::security::{ApiKey, ApiKeyValue, SecurityScheme},
    Modify, OpenApi,
};

use super::{accounts, assets, transactions};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "treasury",
        description = "An asset ledger with consent-based settlement. Every request except GET /health and POST /accounts carries X-Account-Key: the operator key acts as @issuer, an account key acts as that account."
    ),
    paths(
        assets::create,
        assets::list,
        assets::read,
        assets::update,
        assets::archive,
        accounts::create,
        accounts::list,
        accounts::balances,
        accounts::entries,
        transactions::inbox,
        transactions::outbox,
        transactions::create,
        transactions::read,
        transactions::approve,
        transactions::reject,
        transactions::cancel,
    ),
    modifiers(&AccountKey),
    tags(
        (name = "assets"),
        (name = "accounts"),
        (name = "transactions", description = "Mutating requests may carry Idempotency-Key; a replay returns the stored response.")
    )
)]
pub struct ApiDoc;

struct AccountKey;

impl Modify for AccountKey {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "account_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("x-account-key"))),
        );
    }
}
