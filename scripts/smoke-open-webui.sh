#!/usr/bin/env bash
set -euo pipefail

project_name="${COMPOSE_PROJECT_NAME:-no-llm-api-open-webui-smoke}"
export NO_LLM_SCENARIO="${NO_LLM_SCENARIO:-slow}"

cleanup() {
  status=$?
  trap - EXIT
  if [ "$status" -ne 0 ]; then
    docker compose --project-name "$project_name" logs --no-color || true
  fi
  if [ "${KEEP_OPEN_WEBUI_SMOKE:-0}" != "1" ]; then
    docker compose --project-name "$project_name" down --volumes --remove-orphans || true
  fi
  exit "$status"
}
trap cleanup EXIT

docker compose --project-name "$project_name" up --detach --build

for attempt in $(seq 1 180); do
  if curl --fail --silent http://127.0.0.1:3000/health >/dev/null; then
    break
  fi
  if [ "$attempt" -eq 180 ]; then
    echo "Open WebUI did not become healthy within three minutes" >&2
    exit 1
  fi
  sleep 1
done

npm --prefix e2e run test:open-webui

echo "Open WebUI discovered the mock model and rendered the exact streamed fixture answer"
