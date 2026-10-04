#!/bin/sh
set -eu
uname -m
test "$(uname -m)" = aarch64
test -e /proc/self/ns/user
echo ARM_KERNEL_READY
check_cleanup() {
  for process in /proc/[0-9]*/exe; do
    executable=$(readlink "$process" || true)
    case "$executable" in
      /worker/*|*/workers/*|*/sandbox/bin/*|*/sandbox/lib/*)
        echo "ERROR: Worker process leaked: $executable"
        exit 1
        ;;
    esac
  done
}
mkdir -p /tmp/apps
for ecosystem in pip npm go; do
  cp -a "/payload/apps/$ecosystem" "/tmp/apps/$ecosystem"
  program="/tmp/apps/$ecosystem/program"
  if ! "$program" sample.worker.probe '{}' > "/tmp/$ecosystem-first.json" 2> "/tmp/$ecosystem-first.log"; then
    cat "/tmp/$ecosystem-first.json" "/tmp/$ecosystem-first.log"
    exit 1
  fi
  grep -q '"data":true' "/tmp/$ecosystem-first.json"
  check_cleanup
  echo "ARM_WORKER_CALL_PASS=$ecosystem"
  if "$program" sample.worker.reject '{}' > "/tmp/$ecosystem-reject.json" 2> "/tmp/$ecosystem-reject.log"; then
    echo "ERROR: $ecosystem Worker failure unexpectedly succeeded"
    exit 1
  fi
  grep -Fq '"level":"error"' "/tmp/$ecosystem-reject.log"
  grep -Fq 'sample/worker/app.dever:3:' "/tmp/$ecosystem-reject.log"
  grep -Fq 'cannot read component protocol frame header: early eof' "/tmp/$ecosystem-reject.log"
  check_cleanup
  echo "ARM_WORKER_ERROR_PASS=$ecosystem"
  "$program" sample.worker.probe '{}' > "/tmp/$ecosystem-repeat.json"
  grep -q '"data":true' "/tmp/$ecosystem-repeat.json"
  check_cleanup
  cat "/tmp/$ecosystem-first.json"
  echo "ARM_WORKER_PASS=$ecosystem"
  # 只清理当前 guest 用例的可写副本和缓存；原产物保留在只读 payload。
  rm -r /tmp/apps
  mkdir -p /tmp/apps
done
cp -a /payload/apps/http /tmp/apps/http
/tmp/apps/http/application > /tmp/http.log 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT
attempt=0
until wget -q -O /tmp/http.json http://127.0.0.1:18080/sample/http/hello; do
  attempt=$((attempt + 1))
  if test "$attempt" -ge 20; then
    cat /tmp/http.log
    exit 1
  fi
  sleep 1
done
grep -q '"data":"arm-http"' /tmp/http.json
cat /tmp/http.json
kill "$server"
wait "$server"
trap - EXIT
echo ARM_HTTP_PASS
