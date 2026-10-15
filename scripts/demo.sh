#!/usr/bin/env bash
set -euo pipefail

API=${API:-localhost:8888}
OP=${OPERATOR_KEY:-$(grep OPERATOR_KEY .env | cut -d= -f2- | tr -d '"')}
J='content-type: application/json'
BOLD= DIM= RESET=
if [ -t 1 ]; then BOLD=$(tput bold); DIM=$(tput dim); RESET=$(tput sgr0); fi

call() {
    local key=$1 method=$2 path=$3 body=${4:-}
    local out
    if [ -n "$body" ]; then
        out=$(curl -s -X "$method" "$API$path" -H "x-account-key: $key" -H "$J" -d "$body" -w '\n%{http_code}')
    else
        out=$(curl -s -X "$method" "$API$path" -H "x-account-key: $key" -w '\n%{http_code}')
    fi
    STATUS=${out##*$'\n'}
    BODY=${out%$'\n'*}
}

step() {
    printf '\n%s== %s%s  %s->%s %s%s\n' "$BOLD" "$1" "$RESET" "$DIM" "$RESET" "$STATUS" "$(echo "$BODY" | jq -r 'if type != "object" then "" elif .state then " \(.state)" elif .detail then " \(.detail)" else "" end')"
}

show() {
    for who in alice bob; do
        local id key
        if [ "$who" = alice ]; then id=$ALICE_ID; key=$ALICE_KEY; else id=$BOB_ID; key=$BOB_KEY; fi
        call "$key" GET "/accounts/$id/balances"
        echo "$BODY" | jq -r --arg who "$who" '.[] | "   \($who | . + "      " | .[0:6]) \(.asset)  posted \(.posted | tostring | . + "        " | .[0:9]) hold \(.on_hold | tostring | . + "        " | .[0:9]) available \(.available)"'
    done
}

call "$OP" POST /assets '{"code":"USD","name":"US Dollar","scale":2}' >/dev/null
call "$OP" POST /assets '{"code":"XAU","name":"Gold, troy ounces","scale":3}' >/dev/null
call "$OP" POST /accounts '{"name":"alice"}'
ALICE_ID=$(echo "$BODY" | jq -r .id); ALICE_KEY=$(echo "$BODY" | jq -r .key)
call "$OP" POST /accounts '{"name":"bob"}'
BOB_ID=$(echo "$BODY" | jq -r .id); BOB_KEY=$(echo "$BODY" | jq -r .key)

leg() { printf '{"from":"%s","to":"%s","asset":"%s","amount":"%s"}' "$1" "$2" "$3" "$4"; }

call "$OP" POST /transactions "{\"legs\":[$(leg @issuer "$ALICE_ID" USD 1000)]}"
step "1. issuer deposits 1000 USD to alice"
call "$OP" POST /transactions "{\"legs\":[$(leg @issuer "$BOB_ID" XAU 10)]}"
step "2. issuer deposits 10 XAU to bob"
show

call "$ALICE_KEY" POST /transactions "{\"legs\":[$(leg "$ALICE_ID" "$BOB_ID" USD 100)]}"
step "3. alice sends 100 USD to bob: one debtor, settles at once"
show

call "$ALICE_KEY" POST /transactions "{\"memo\":\"100 USD for 1.5 XAU\",\"legs\":[$(leg "$ALICE_ID" "$BOB_ID" USD 100),$(leg "$BOB_ID" "$ALICE_ID" XAU 1.5)]}"
SWAP=$(echo "$BODY" | jq -r .id)
step "4. alice offers 100 USD for 1.5 XAU: her USD is on hold, bob's XAU is not"
show

call "$BOB_KEY" GET "/accounts/$BOB_ID/inbox"
step "5. bob reads his inbox"
echo "$BODY" | jq -r '.[] | "   \(.memo) from \(.proposer): \(.required_approvals | map("\(.account | .[0:8]) \(.decision)") | join(", "))"'

call "$ALICE_KEY" POST "/transactions/$SWAP/approve"
step "6. alice approves her own offer"

call "$BOB_KEY" POST "/transactions/$SWAP/approve"
step "7. bob approves: both legs settle, or neither"
show

call "$BOB_KEY" POST "/transactions/$SWAP/approve"
step "8. bob approves again"

call "$ALICE_KEY" POST /transactions "{\"memo\":\"1 USD for 100 XAU\",\"legs\":[$(leg "$ALICE_ID" "$BOB_ID" USD 1),$(leg "$BOB_ID" "$ALICE_ID" XAU 100)]}"
OFFER=$(echo "$BODY" | jq -r .id)
step "9. alice offers 1 USD for 100 XAU"
call "$BOB_KEY" POST "/transactions/$OFFER/reject"
step "10. bob rejects: the hold is released"
show

call "$ALICE_KEY" POST /transactions "{\"memo\":\"600 USD for 1 XAU\",\"legs\":[$(leg "$ALICE_ID" "$BOB_ID" USD 600),$(leg "$BOB_ID" "$ALICE_ID" XAU 1)]}"
HOLD=$(echo "$BODY" | jq -r .id)
step "11. alice offers 600 USD for 1 XAU"
call "$ALICE_KEY" POST /transactions "{\"legs\":[$(leg "$ALICE_ID" "$BOB_ID" USD 600)]}"
step "12. alice tries to send the same 600 USD elsewhere"
call "$ALICE_KEY" POST "/transactions/$HOLD/cancel"
step "13. alice cancels the offer"

call "$ALICE_KEY" POST /transactions "{\"expires_in_secs\":1,\"legs\":[$(leg "$ALICE_ID" "$BOB_ID" USD 50),$(leg "$BOB_ID" "$ALICE_ID" XAU 1)]}"
TTL=$(echo "$BODY" | jq -r .id)
step "14. alice offers 50 USD with a one second TTL"
show
sleep 2
call "$ALICE_KEY" GET "/transactions/$TTL"
step "15. two seconds later"
show

call "$ALICE_KEY" GET "/accounts/$ALICE_ID/entries"
step "16. alice's journal: every entry names the proposal that authorised it"
echo "$BODY" | jq -r '.[] | "   \(.asset)  \(.amount | tostring | ("          " + .) | .[-10:])  \(.proposal_id)"'
