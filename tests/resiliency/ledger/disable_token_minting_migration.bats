# Resiliency test verifying the Disable Token Minting Migration

GIT_ROOT="$BATS_TEST_DIRNAME/../../../"
MIGRATION_ROOT="$GIT_ROOT/staging/ledger_migrations.json"
MAKEFILE="Makefile.ledger"
MFX_ADDRESS=mqbh742x4s356ddaryrxaowt4wxtlocekzpufodvowrirfrqaaaaa3l

load '../../test_helper/load'
load '../../test_helper/ledger'

function setup() {
    load "test_helper/bats-assert/load"
    load "test_helper/bats-support/load"

    mkdir "$BATS_TEST_ROOTDIR"

    # jq doesn't support bigint
    jq '(.migrations[] | select(.name == "Token Migration")).block_height |= 30 |
        (.migrations[] | select(.name == "Token Migration")).disabled |= empty |
        (.migrations[] | select(.name == "Disable Token Mint Migration")).block_height |= 35 |
        (.migrations[] | select(.name == "Disable Token Mint Migration")).disabled |= empty |
        (.migrations[] | select(.name == "Token Migration")) |= . + {
            "token_identity": "'$(identity 1)'",
            "token_next_subresource": 0,
            "symbol": "'${MFX_ADDRESS}'",
            "symbol_name": "Manifest Network Token",
            "symbol_decimals": 9,
            "symbol_total": 100000000000000,
            "symbol_circulating": 100000000000000,
            "symbol_maximum": null,
            "symbol_owner": "'${MFX_ADDRESS}'"
        }' \
        "$MIGRATION_ROOT" > "$BATS_TEST_ROOTDIR/migrations.json"

    cp "$GIT_ROOT/staging/ledger_state.json5" "$BATS_TEST_ROOTDIR/ledger_state.json5"

    # Use production MFX address
    sed -i.bak 's/mqbfbahksdwaqeenayy2gxke32hgb7aq4ao4wt745lsfs6wiaaaaqnz/mqbh742x4s356ddaryrxaowt4wxtlocekzpufodvowrirfrqaaaaa3l/g' "$BATS_TEST_ROOTDIR/ledger_state.json5"

    # Skip hash check
    sed -i.bak 's/hash/\/\/hash/' "$BATS_TEST_ROOTDIR/ledger_state.json5"

    (
      cd "$GIT_ROOT/docker/" || exit
      make -f $MAKEFILE clean
      make -f $MAKEFILE start-nodes-detached \
          ID_WITH_BALANCES="$(identity 1):1000000" \
          STATE="$BATS_TEST_ROOTDIR/ledger_state.json5" \
          TOKEN="$MFX_ADDRESS" \
          MIGRATIONS="$BATS_TEST_ROOTDIR/migrations.json" || {
        echo '# Could not start nodes...' >&3
        exit 1
      }
    ) > /dev/null

    # Give time to the servers to start.
    wait_for_server 8000 8001 8002 8003
}

function teardown() {
    (
      cd "$GIT_ROOT/docker/" || exit 1
      make -f $MAKEFILE stop-nodes
    ) 2> /dev/null

    # Fix for BATS verbose run/test output gathering
    cd "$GIT_ROOT/tests/resiliency/ledger" || exit 1
}

@test "$SUITE: Disable Token Minting Migration" {
    check_consistency --pem=1 --balance=1000000 --id="$(identity 1)" 8000 8001 8002 8003

    # Token endpoints should be disabled
    call_ledger --pem=1 --port=8000 token info ${MFX_ADDRESS}
    assert_output --partial "Invalid method name"

    # Enable Token Migration
    wait_for_block 30

    # Token endpoints should be enabled. Mint some tokens
    call_ledger --pem=1 --port=8000 token mint MFX ''\''{"'$(identity 2)'": 123, "'$(identity 3)'": 456}'\'''
    check_consistency --pem=1 --balance=1000000 --id="$(identity 1)" 8000 8001 8002 8003
    check_consistency --pem=2 --balance=123 8000 8001 8002 8003
    check_consistency --pem=3 --balance=456 8000 8001 8002 8003

    # Create a new token
    create_token --pem=1 --port=8000
    call_ledger --pem=1 --port=8000 token update --name "\"ZZZ name\"" \
        --ticker "ZZZ" \
        --decimals "6" \
        --memo "\"Update memo\"" \
        --owner "$(identity 2)" \
        "${SYMBOL}"

    # Disable Token Minting
    wait_for_block 35

    # MFX minting should still work
    call_ledger --pem=1 --port=8000 token mint MFX ''\''{"'$(identity 2)'": 123, "'$(identity 3)'": 456}'\'''
    refute_output --partial "Token minting is disabled on this network"
    check_consistency --pem=1 --balance=1000000 --id="$(identity 1)" 8000 8001 8002 8003
    check_consistency --pem=2 --balance=246 8000 8001 8002 8003
    check_consistency --pem=3 --balance=912 8000 8001 8002 8003

    # Token burn should still work
    call_ledger --pem=1 --port=8000 token burn MFX ''\''{"'$(identity 2)'": 246, "'$(identity 3)'": 912}'\''' --error-on-under-burn
    check_consistency --pem=2 --balance=0 8000 8001 8002 8003
    check_consistency --pem=3 --balance=0 8000 8001 8002 8003

    # ZZZ minting should fail
    call_ledger --pem=1 --port=8000 token mint ZZZ ''\''{"'$(identity 2)'": 123, "'$(identity 3)'": 456}'\'''
    assert_output --partial "Token minting is disabled on this network"

    call_ledger --port=8000 token info "${SYMBOL}"
    assert_output --regexp "total:.*(.*0,.*)"
    assert_output --regexp "circulating:.*(.*0,.*)"
}
