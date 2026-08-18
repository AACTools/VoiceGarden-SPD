#!/bin/sh
# End-to-end synthesis check for a VoiceGarden-SPD install.
#
# Drives the same rust-tts-wrapper synthesis path the speech-dispatcher
# module runs and reports PASS/FAIL per check:
#
#   1. every installed local sherpa-onnx model (bytes of PCM produced)
#   2. edge (credential-free), including a bare <speak> envelope — the
#      speech-dispatcher SSML shape that used to synthesise zero audio
#   3. azure / google / openai when credentials are available: an
#      engines.json entry already, or env vars (added + live-verified
#      non-interactively when set)
#   4. the speech-dispatcher daemon view, when a daemon is running
#
# Usage:
#   scripts/e2e-check.sh [--text "sentence"] [--runs N]
#
# Env:
#   VGSPD_BIN            voicegarden-spd binary (default: on PATH, then
#                       ./target/release/voicegarden-spd)
#   MICROSOFT_TOKEN,
#   MICROSOFT_REGION     Azure Speech key + region
#   GOOGLE_API_KEY       Google Cloud TTS API key
#   OPENAI_API_KEY       OpenAI API key
#
# Engines without credentials are SKIPPED (not failures). Exits non-zero
# when any attempted check fails.

set -u

TEXT="The quick brown fox jumps over the lazy dog."
SSML_TEXT="<speak>The quick brown fox jumps over the lazy dog.</speak>"
RUNS=1

while [ $# -gt 0 ]; do
    case "$1" in
        --text) TEXT=${2-}; shift 2 ;;
        --runs) RUNS=${2-}; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

# --- locate the CLI ---------------------------------------------------------
VG="${VGSPD_BIN:-}"
if [ -z "$VG" ]; then
    if command -v voicegarden-spd >/dev/null 2>&1; then
        VG=$(command -v voicegarden-spd)
    elif [ -x "./target/release/voicegarden-spd" ]; then
        VG="./target/release/voicegarden-spd"
    else
        echo "FAIL: voicegarden-spd not found — set VGSPD_BIN, install it, or build with cargo" >&2
        exit 1
    fi
fi

PASS=0
FAIL=0

result() { # result PASS|SKIP|FAIL label [detail]
    printf '  [%s] %s%s\n' "$1" "$2" "${3+: $3}"
    case "$1" in
        PASS) PASS=$((PASS + 1)) ;;
        FAIL) FAIL=$((FAIL + 1)) ;;
    esac
}

# bench one voice; PASS when it reports a positive byte count.
check_voice() { # check_voice label voice text
    out=$("$VG" bench "$2" "$3" "$RUNS" 2>&1)
    status=$?
    bytes=$(printf '%s\n' "$out" | sed -n 's/.*(\([0-9][0-9]*\) bytes).*/\1/p' | head -1)
    if [ "$status" -eq 0 ] && [ -n "$bytes" ] && [ "$bytes" -gt 0 ]; then
        result PASS "$1" "$bytes bytes"
    else
        result FAIL "$1" "$(printf '%s' "$out" | head -1)"
    fi
}

# First field of a `voice search` data row (local voices contain '#').
first_voice() { # first_voice search-args...
    "$VG" voice search "$@" 2>/dev/null | awk 'NR>1 && $1 != "" && $1 !~ /^→/ {print $1; exit}'
}

echo "VoiceGarden-SPD e2e check — $($VG --version 2>/dev/null || echo "$VG")"
echo

# --- 1. local models --------------------------------------------------------
echo "local sherpa-onnx models:"
locals=$("$VG" voice search --source local 2>/dev/null | awk 'NR>1 && $1 ~ /#/ {print $1}')
if [ -z "$locals" ]; then
    result SKIP "no local models installed (see voicegarden-spd model find)"
else
    for v in $locals; do
        check_voice "model $v" "$v" "$TEXT"
    done
fi
echo

# --- 2. edge (credential-free) ----------------------------------------------
echo "edge (no credentials needed):"
edge_voice=$(first_voice --engine edge --lang en-GB)
[ -n "$edge_voice" ] || edge_voice=$(first_voice --engine edge)
if [ -z "$edge_voice" ]; then
    # no voice cache yet — try a refresh, then look again
    "$VG" refresh edge >/dev/null 2>&1
    edge_voice=$(first_voice --engine edge --lang en-GB)
fi
if [ -n "$edge_voice" ]; then
    check_voice "edge $edge_voice" "$edge_voice" "$TEXT"
    # The speech-dispatcher SSML shape: a bare <speak> envelope used to
    # synthesise zero audio on Azure/Edge (silent success).
    check_voice "edge SSML envelope" "$edge_voice" "$SSML_TEXT"
else
    result FAIL "edge voice discovery" "no edge voices in the cache and refresh failed"
fi
echo

# --- 3. credentialed cloud engines -------------------------------------------
engine_configured() { # engine_configured id -> prints yes/no
    # The display name can be multiple words ("Google Cloud"), so match
    # "configured" in any field after the id.
    "$VG" engine list 2>/dev/null | awk -v id="$1" '
        $1 == id {
            for (i = 2; i <= NF; i++)
                if ($i == "configured") { print "yes"; exit }
            print "no"; exit
        }'
}

echo "azure:"
if [ "$(engine_configured azure)" = "yes" ]; then
    azure_voice=$(first_voice --engine azure --lang en-GB)
    [ -n "$azure_voice" ] || azure_voice=$(first_voice --engine azure)
    if [ -n "$azure_voice" ]; then
        check_voice "azure $azure_voice" "$azure_voice" "$TEXT"
        check_voice "azure SSML envelope" "$azure_voice" "$SSML_TEXT"
    else
        result SKIP "azure" "configured but no cached voices — run voicegarden-spd refresh"
    fi
elif [ -n "${MICROSOFT_TOKEN:-}" ] && [ -n "${MICROSOFT_REGION:-}" ]; then
    if "$VG" engine add azure --set "subscriptionKey=$MICROSOFT_TOKEN" \
            --set "region=$MICROSOFT_REGION" >/dev/null 2>&1; then
        azure_voice=$(first_voice --engine azure --lang en-GB)
        [ -n "$azure_voice" ] || azure_voice=$(first_voice --engine azure)
        check_voice "azure $azure_voice (from env creds)" "$azure_voice" "$TEXT"
    else
        result FAIL "azure engine add" "live verification rejected the credentials"
    fi
else
    result SKIP "azure" "not configured; set MICROSOFT_TOKEN and MICROSOFT_REGION to test"
fi
echo

echo "google:"
if [ "$(engine_configured google)" = "yes" ]; then
    google_voice=$(first_voice --engine google --lang en-GB)
    [ -n "$google_voice" ] || google_voice=$(first_voice --engine google)
    if [ -n "$google_voice" ]; then
        check_voice "google $google_voice" "$google_voice" "$TEXT"
    else
        result SKIP "google" "configured but no cached voices — run voicegarden-spd refresh"
    fi
elif [ -n "${GOOGLE_API_KEY:-}" ]; then
    if "$VG" engine add google --set "apiKey=$GOOGLE_API_KEY" >/dev/null 2>&1; then
        google_voice=$(first_voice --engine google --lang en-GB)
        [ -n "$google_voice" ] || google_voice=$(first_voice --engine google)
        check_voice "google $google_voice (from env creds)" "$google_voice" "$TEXT"
    else
        result FAIL "google engine add" "live verification rejected the credentials"
    fi
else
    result SKIP "google" "not configured; set GOOGLE_API_KEY to test"
fi
echo

echo "openai:"
if [ "$(engine_configured openai)" = "yes" ]; then
    openai_voice=$(first_voice --engine openai)
    if [ -n "$openai_voice" ]; then
        check_voice "openai $openai_voice" "$openai_voice" "$TEXT"
    else
        result SKIP "openai" "configured but no cached voices — run voicegarden-spd refresh"
    fi
elif [ -n "${OPENAI_API_KEY:-}" ]; then
    if "$VG" engine add openai --set "apiKey=$OPENAI_API_KEY" >/dev/null 2>&1; then
        openai_voice=$(first_voice --engine openai)
        check_voice "openai $openai_voice (from env creds)" "$openai_voice" "$TEXT"
    else
        result FAIL "openai engine add" "live verification rejected the credentials"
    fi
else
    result SKIP "openai" "not configured; set OPENAI_API_KEY to test"
fi
echo

# --- 4. daemon view (best effort) --------------------------------------------
if command -v spd-say >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
    echo "speech-dispatcher daemon:"
    if daemon_voices=$(timeout 10 spd-say -o voicegarden-spd -L 2>/dev/null | awk 'NR>1' | wc -l) \
        && [ "$daemon_voices" -gt 0 ] 2>/dev/null; then
        result PASS "daemon voice list" "$daemon_voices voices via spd-say"
    else
        result SKIP "daemon voice list" "no running daemon answering for this module"
    fi
    echo
fi

echo "--------------------------------------------------"
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
