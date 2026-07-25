#!/usr/bin/env bash
# Fetch the upstream OpenAI OpenAPI specification and record its provenance.
#
# Downloads openapi.yaml from openai/openai-openapi into the repository root and
# writes openapi.provenance.json with the upstream commit, fetch time and
# sha256. Upstream keeps info.version pinned at 2.3.0 across unrelated changes,
# so the commit sha and sha256 are the only reliable staleness signals.
#
# Usage:
#   scripts/fetch-openapi.sh            # fetch and record
#   scripts/fetch-openapi.sh --check    # report staleness, exit 1 if stale
#   REF=some-sha scripts/fetch-openapi.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
spec_path="$repo_root/openapi.yaml"
provenance_path="$repo_root/openapi.provenance.json"
ref="${REF:-main}"
raw_url="https://raw.githubusercontent.com/openai/openai-openapi/$ref/openapi.yaml"
api_url="https://api.github.com/repos/openai/openai-openapi/commits?path=openapi.yaml&sha=$ref&per_page=1"

json_field() { sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1" | head -n 1; }

commit_json="$(mktemp)"
trap 'rm -f "$commit_json"' EXIT
curl -fsSL -H 'User-Agent: no-llm-api-spec-fetch' "$api_url" -o "$commit_json"
upstream_sha="$(json_field "$commit_json" sha)"
upstream_date="$(json_field "$commit_json" date)"

if [ "${1:-}" = "--check" ]; then
  if [ ! -f "$provenance_path" ]; then
    echo "no openapi.provenance.json; run scripts/fetch-openapi.sh" >&2
    exit 1
  fi
  local_sha="$(json_field "$provenance_path" upstream_commit)"
  if [ "$local_sha" = "$upstream_sha" ]; then
    echo "up to date at ${upstream_sha:0:12} ($upstream_date)"
    exit 0
  fi
  echo "STALE: local ${local_sha:0:12} vs upstream ${upstream_sha:0:12} ($upstream_date)" >&2
  exit 1
fi

tmp_spec="$(mktemp)"
trap 'rm -f "$commit_json" "$tmp_spec"' EXIT
curl -fsSL "$raw_url" -o "$tmp_spec"

sha256="$(sha256sum "$tmp_spec" | cut -d' ' -f1)"
openapi_version="$(sed -n 's/^openapi:[[:space:]]*\(.*\)$/\1/p' "$tmp_spec" | head -n 1)"
info_version="$(sed -n 's/^[[:space:]]\+version:[[:space:]]*\(.*\)$/\1/p' "$tmp_spec" | head -n 1)"

mv "$tmp_spec" "$spec_path"
bytes="$(wc -c < "$spec_path" | tr -d ' ')"

cat > "$provenance_path" <<JSON
{
  "source": "https://github.com/openai/openai-openapi/blob/$ref/openapi.yaml",
  "upstream_commit": "$upstream_sha",
  "upstream_commit_date": "$upstream_date",
  "fetched_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "sha256": "$sha256",
  "bytes": $bytes,
  "openapi_version": "$openapi_version",
  "info_version": "$info_version"
}
JSON

echo "wrote openapi.yaml ($bytes bytes, OpenAPI $openapi_version, info.version $info_version)"
echo "upstream ${upstream_sha:0:12} $upstream_date"
