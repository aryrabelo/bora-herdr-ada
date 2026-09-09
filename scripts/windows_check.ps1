param(
    [ValidateSet("lint", "check")]
    [string]$Mode = "check"
)

$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param([string]$Command, [string[]]$Arguments)

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "command failed with exit code $LASTEXITCODE`: $Command $($Arguments -join ' ')"
    }
}

function Invoke-CargoWithZigCacheRecovery {
    param([string[]]$Arguments)

    & cargo @Arguments
    if ($LASTEXITCODE -eq 0) {
        return
    }

    Write-Warning "cargo compile failed; clearing Zig build caches and retrying once"
    Remove-Item -Recurse -Force .zig-cache -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force vendor/libghostty-vt/.zig-cache -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force vendor/libghostty-vt/zig-out -ErrorAction SilentlyContinue
    Invoke-Checked cargo $Arguments
}

Invoke-Checked cargo @("fmt", "--check")
Invoke-CargoWithZigCacheRecovery @(
    "clippy",
    "--bin",
    "bora",
    "--locked",
    "--",
    "-D",
    "warnings",
    "-A",
    "clippy::unwrap_used"
)

# AGENTS.md Code Conventions: "no `unwrap()` in production code". Mirrors the
# production-only gate in the Unix `just lint` recipe. The run above compiles
# only the bin target already, so the two are the same scope here; keeping the
# deny on its own line matches the Unix recipe and fails loudly on its own.
Invoke-CargoWithZigCacheRecovery @(
    "clippy",
    "--bin",
    "bora",
    "--locked",
    "--target",
    "x86_64-pc-windows-msvc",
    "--",
    "-D",
    "clippy::unwrap_used"
)

if ($Mode -eq "lint") {
    return
}

Invoke-Checked just @("test")
Invoke-Checked cargo @("build", "--locked")
