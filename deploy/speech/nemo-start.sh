#!/bin/sh
set -eu
umask 077

nemo=/opt/nemo/bin/nemo-speech
socket=/run/speech/nemo.sock
port=8191
test -r /models/nemotron-3.5-asr-streaming-0.6b.q8_0.gguf
test "$(stat -c '%a' /run/speech)" = 700
test "$(stat -c '%u' /run/speech)" = "$(id -u)"
if [ -S "$socket" ] && [ ! -L "$socket" ]; then
    unlink "$socket"
fi

"$nemo" serve --host 127.0.0.1 --port "$port" --device cpu --max-upload-mb 8 --no-ui \
  --asr-model /models/nemotron-3.5-asr-streaming-0.6b.q8_0.gguf &
engine=$!
bridge=
cleanup() {
    if [ -n "$bridge" ]; then
        kill "$bridge" 2>/dev/null || :
        wait "$bridge" 2>/dev/null || :
    fi
    kill "$engine" 2>/dev/null || :
    wait "$engine" 2>/dev/null || :
    if [ -S "$socket" ] && [ ! -L "$socket" ]; then
        unlink "$socket"
    fi
}
trap cleanup EXIT
trap 'exit 0' TERM INT

until "$nemo" health --url "http://127.0.0.1:$port/ready" >/dev/null 2>&1; do
    kill -0 "$engine" 2>/dev/null || exit 1
    sleep 1
done
socat "UNIX-LISTEN:$socket,fork,max-children=12,mode=0600" "TCP4:127.0.0.1:$port,connect-timeout=2" &
bridge=$!
while kill -0 "$engine" 2>/dev/null && kill -0 "$bridge" 2>/dev/null; do
    sleep 1
done
exit 1
