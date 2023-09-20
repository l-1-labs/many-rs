GIT_ROOT="$BATS_TEST_DIRNAME/../../../"

load '../../test_helper/load'
load '../../test_helper/web'
load '../../test_helper/kvstore'
load '../../test_helper/http-proxy'

function setup() {
    load "test_helper/bats-assert/load"
    load "test_helper/bats-support/load"

    mkdir "$BATS_TEST_ROOTDIR"

    skip_if_missing_background_utilities

    start_web --pem "$(pem 0)" --domain ghostcloud.org

    xxd -p -r << EOF > test_dweb.zip
504b03040a0300000000af680857dbff951917000000170000000a000000
696e6465782e68746d6c3c68313e48656c6c6f20466f6f626172213c2f68
313e0a504b01023f030a0300000000af680857dbff951917000000170000
000a0024000000000000002080a48100000000696e6465782e68746d6c0a
00200000000000010018000029f7881acad9010029f7881acad9010029f7
881acad901504b050600000000010001005c0000003f0000000000
EOF
}

function teardown() {
    stop_background_run
}

@test "$SUITE: dweb website deployment works" {
    call_web --pem=1 --port=8000 deploy test_dweb test_dweb.zip
    assert_output --partial "https://test_dweb-$(identity 1).ghostcloud.org"
}

@test "$SUITE: dweb website deployment fails if owner is not sender" {
    call_web --pem=1 --port=8000 deploy test_dweb test_dweb.zip --owner "$(identity 2)"
    assert_output --partial "Invalid owner: $(identity 2)"
}

@test "$SUITE: dweb website update works" {
    call_web --pem=1 --port=8000 deploy test_dweb test_dweb.zip
    assert_output --partial "https://test_dweb-$(identity 1).ghostcloud.org"

    call_web --pem=1 --port=8000 update test_dweb test_dweb.zip
    assert_output --partial "https://test_dweb-$(identity 1).ghostcloud.org"
}

@test "$SUITE: dweb website update fails if owner is not sender" {
    call_web --pem=1 --port=8000 deploy test_dweb test_dweb.zip
    assert_output --partial "https://test_dweb-$(identity 1).ghostcloud.org"

    call_web --pem=1 --port=8000 update test_dweb test_dweb.zip --owner "$(identity 2)"
    assert_output --partial "Invalid owner: $(identity 2)"
}

@test "$SUITE: dweb website update fails if nonexistent" {
    call_web --pem=1 --port=8000 update test_dweb test_dweb.zip --owner "$(identity 1)"
    assert_output --partial "Nonexistent site: test_dweb"
}

@test "$SUITE: dweb website removal works" {
    call_web --pem=1 --port=8000 deploy test_dweb test_dweb.zip
    assert_output  --partial "https://test_dweb-$(identity 1).ghostcloud.org"
    call_web --pem=1 --port=8000 remove test_dweb
    call_web --pem=1 --port=8000 list
    assert_output '{0: []}'
}

@test "$SUITE: dweb website listing works" {
    call_web --pem=1 --port=8000 deploy test_dweb test_dweb.zip
    assert_output  --partial "https://test_dweb-$(identity 1).ghostcloud.org"
    call_web --pem=2 --port=8000 deploy foobar test_dweb.zip
    assert_output  --partial "https://foobar-$(identity 2).ghostcloud.org"
    call_web --pem=1 --port=8000 list
    assert_output --partial 'test_dweb'
    assert_output --partial 'foobar'
    assert_output --partial "$(identity 1)"
    assert_output --partial "$(identity 2)"
}

@test "$SUITE: dweb get must start by /http" {
    call_web --pem=1 --port=8000 deploy test_dweb test_dweb.zip
    assert_output  --partial "https://test_dweb-$(identity 1).ghostcloud.org"

    # Call the kvstore endpoint of many-web
    call_kvstore --pem=1 --port=8000 get "foobar"
    assert_output --partial "Key should start with '/http/'."

    call_kvstore --pem=1 --port=8000 get "/http/$(identity 1)/test_dweb/index.html"
    assert_output --partial 'Hello Foobar!'
}

@test "$SUITE: check for index.html" {
    xxd -p -r << EOF > dummy.zip
    504b03040a000000000043540e5716359631060000000600000006001c00
666f6f62617255540900035d3bda645d3bda6475780b000104e803000004
e803000048656c6c6f0a504b01021e030a000000000043540e5716359631
0600000006000000060018000000000000000000a48100000000666f6f62
617255540500035d3bda6475780b000104e803000004e8030000504b0506
00000000010001004c000000460000000000
EOF
    call_web --pem=1 --port=8000 deploy dummy dummy.zip
    assert_output --partial "Missing 'index.html' at the root of the archive."
}