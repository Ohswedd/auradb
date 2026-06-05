# Generate development peer certificates for the local three-node AuraDB
# multi-node preview cluster (docker-compose.cluster.yml) on Windows.
#
# DEVELOPMENT ONLY. The generated CA and node certificates are self-signed and
# unencrypted; never use them in production. Single-node mode remains the
# recommended production mode.
#
# Produces, under examples/cluster/certs/ (git-ignored): ca.crt/ca.key and
# node1/node2/node3 .crt/.key signed by that CA, each with SANs covering its
# service name plus localhost and 127.0.0.1.
#
# Usage (from the repo root or anywhere):
#   pwsh examples/cluster/generate-dev-certs.ps1
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir "..\..")).Path
$CertsDir = Join-Path $ScriptDir "certs"

if ($env:AURADB) {
    $Auradb = $env:AURADB
} elseif (Get-Command auradb -ErrorAction SilentlyContinue) {
    $Auradb = "auradb"
} else {
    Write-Host "building auradb (cargo build -p auradb-cli)..."
    Push-Location $RepoRoot
    cargo build -p auradb-cli | Out-Null
    Pop-Location
    $Auradb = Join-Path $RepoRoot "target\debug\auradb.exe"
}

New-Item -ItemType Directory -Force -Path $CertsDir | Out-Null

foreach ($node in @("node1", "node2", "node3")) {
    & $Auradb cert generate-dev `
        --out-dir $CertsDir `
        --server-name $node `
        --san $node `
        --san localhost `
        --san 127.0.0.1
}

Write-Host ""
Write-Host "Development peer certificates written to $CertsDir"
Get-ChildItem $CertsDir | Select-Object -ExpandProperty Name
Write-Host ""
Write-Host "DEVELOPMENT ONLY - do not use these certificates in production."
Write-Host "Next: set the same peer_auth_token in examples/cluster/docker/node{1,2,3}.toml,"
Write-Host "then: docker compose -f docker-compose.cluster.yml up -d"
