#!/bin/sh
set -eu
# Only this container's network namespace is changed. Privileges are then dropped.
tc qdisc add dev eth0 root netem limit 20000 delay "${BENCH_DELAY_MS:-0}ms" "${BENCH_JITTER_MS:-0}ms" loss "${BENCH_LOSS_PERCENT:-0}%" rate "${BENCH_MBIT:-500}mbit"
exec setpriv --reuid=1000 --regid=1000 --clear-groups --bounding-set=-all --inh-caps=-all --ambient-caps=-all --no-new-privs /bin/sh -c '
    if [ "$1" = serve ]; then
        if [ "${BENCH_TLS:-direct}" = direct ]; then
            exec /bench/link-mailbox-bench serve
        fi
        request_buffering=off
        response_buffering=off
        ws_gate=""
        case "$BENCH_TLS" in
            nginx-upload-buffered) request_buffering=on ;;
            nginx-response-buffered) response_buffering=on ;;
            nginx-no-upgrade) ws_gate="return 403;" ;;
        esac
        sed -e "s/__REQUEST_BUFFERING__/$request_buffering/" \
            -e "s/__RESPONSE_BUFFERING__/$response_buffering/" \
            -e "s/__WS_GATE__/$ws_gate/" /bench/nginx.conf > /tmp/nginx.conf
        nginx -c /tmp/nginx.conf -g "daemon off;" >/dev/null 2>&1 &
        proxy=$!
        /bench/link-mailbox-bench serve >/dev/null 2>&1 &
        relay=$!
        trap "kill $proxy $relay 2>/dev/null || true" EXIT INT TERM
        wait "$relay"
    else
        exec /bench/link-mailbox-bench "$@"
    fi
' bench "$@"
