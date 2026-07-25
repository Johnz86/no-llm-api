<#
.SYNOPSIS
Fetch the upstream OpenAI OpenAPI specification and record its provenance.

.DESCRIPTION
Downloads openapi.yaml from openai/openai-openapi, writes it to the repository
root, and records the upstream commit, fetch time and sha256 in
openapi.provenance.json so a reader can tell how old the local copy is.

Upstream keeps info.version pinned at 2.3.0 across unrelated changes, so the
commit sha and sha256 are the only reliable staleness signals.

.PARAMETER Check
Compare the recorded provenance against upstream without writing anything.
Exits 1 when the local copy is stale. Intended for CI.

.PARAMETER Ref
Upstream git ref to fetch. Defaults to main, the upstream default branch.
Note that raw.githubusercontent.com also still serves a legacy master branch,
but the commits API only knows main.
#>
[CmdletBinding()]
param(
    [switch]$Check,
    [string]$Ref = 'main'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$specPath = Join-Path $repoRoot 'openapi.yaml'
$provenancePath = Join-Path $repoRoot 'openapi.provenance.json'
$headers = @{ 'User-Agent' = 'no-llm-api-spec-fetch' }
$rawUrl = "https://raw.githubusercontent.com/openai/openai-openapi/$Ref/openapi.yaml"

function Get-UpstreamCommit {
    $uri = "https://api.github.com/repos/openai/openai-openapi/commits?path=openapi.yaml&sha=$Ref&per_page=1"
    $commit = (Invoke-RestMethod -Uri $uri -Headers $headers)[0]
    [pscustomobject]@{
        Sha     = $commit.sha
        Date    = $commit.commit.author.date
        Subject = $commit.commit.message.Split("`n")[0]
    }
}

$upstream = Get-UpstreamCommit

if ($Check) {
    if (-not (Test-Path $provenancePath)) {
        Write-Host "no openapi.provenance.json; run scripts/fetch-openapi.ps1" -ForegroundColor Yellow
        exit 1
    }
    $local = Get-Content $provenancePath -Raw | ConvertFrom-Json
    if ($local.upstream_commit -eq $upstream.Sha) {
        Write-Host "up to date at $($upstream.Sha.Substring(0,12)) ($($upstream.Date))"
        exit 0
    }
    Write-Host "STALE: local $($local.upstream_commit.Substring(0,12)) ($($local.upstream_commit_date)) vs upstream $($upstream.Sha.Substring(0,12)) ($($upstream.Date))" -ForegroundColor Yellow
    Write-Host "upstream head: $($upstream.Subject)"
    exit 1
}

$temp = New-TemporaryFile
try {
    Invoke-WebRequest -Uri $rawUrl -OutFile $temp -UseBasicParsing
    $hash = (Get-FileHash $temp -Algorithm SHA256).Hash.ToLower()
    $head = Get-Content $temp -TotalCount 10
    $openapiVersion = ($head | Select-String -Pattern '^openapi:\s*(\S+)').Matches[0].Groups[1].Value
    $infoVersion = ($head | Select-String -Pattern '^\s+version:\s*(\S+)').Matches[0].Groups[1].Value

    Move-Item -Force $temp $specPath

    [pscustomobject][ordered]@{
        source               = "https://github.com/openai/openai-openapi/blob/$Ref/openapi.yaml"
        upstream_commit      = $upstream.Sha
        upstream_commit_date = $upstream.Date
        upstream_subject     = $upstream.Subject
        fetched_at           = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        sha256               = $hash
        bytes                = (Get-Item $specPath).Length
        openapi_version      = $openapiVersion
        info_version         = $infoVersion
    } | ConvertTo-Json | Set-Content -Path $provenancePath -Encoding utf8

    Write-Host "wrote openapi.yaml ($([math]::Round((Get-Item $specPath).Length / 1MB, 2)) MB, OpenAPI $openapiVersion, info.version $infoVersion)"
    Write-Host "upstream $($upstream.Sha.Substring(0,12)) $($upstream.Date)"
}
finally {
    if (Test-Path $temp) { Remove-Item $temp -Force }
}
