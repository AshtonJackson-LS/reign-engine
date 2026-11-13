# Makes a new Reign release: asks for the version, puts it in Cargo.toml,
# commits, tags and pushes. GitHub Actions then tests, builds, signs and
# publishes it (see RELEASING.md). Run it through release.bat.
#
#   scripts\release.ps1                 ask for the version
#   scripts\release.ps1 -Version 0.2.0  no question
#   scripts\release.ps1 -DryRun         do everything except push
param([string]$Version, [switch]$DryRun)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Stop-Release($message) {
    Write-Host ""
    Write-Host "Stopped: $message" -ForegroundColor Red
    exit 1
}
function Ask-Yes($question) {
    (Read-Host "$question (y/N)") -match '^(y|yes)$'
}
function Parse-Version($text) {
    if ($text -notmatch '^\d+\.\d+\.\d+$') { return $null }
    [version]$text
}

# 1. The repository is ready.
if (-not (Get-Command git -ErrorAction SilentlyContinue)) { Stop-Release 'git is not installed.' }
if (git status --porcelain) {
    git status --short
    Stop-Release 'you have uncommitted changes. Commit them (or stash them) first.'
}
$branch = git branch --show-current
if ($branch -ne 'main') {
    Write-Host "You're on the branch '$branch'. Releases are normally made from 'main'." -ForegroundColor Yellow
    if (-not (Ask-Yes 'Release from this branch anyway?')) { Stop-Release 'switch to main first: git checkout main' }
}
Write-Host 'Checking GitHub for newer commits...'
git fetch --quiet origin
if ($LASTEXITCODE -ne 0) { Stop-Release "can't reach GitHub (git fetch failed)." }
$upstream = git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>$null
if ($upstream) {
    $behind = [int](git rev-list --count "HEAD..$upstream")
    if ($behind -gt 0) { Stop-Release "GitHub has $behind commit(s) you don't. Run: git pull" }
}

# 2. The version.
$manifest = Get-Content Cargo.toml -Raw
if ($manifest -notmatch '(?ms)^\[workspace\.package\]\s*\r?\nversion = "(\d+\.\d+\.\d+)"') {
    Stop-Release 'no version under [workspace.package] in Cargo.toml.'
}
$current = $Matches[1]
$c = Parse-Version $current
$suggested = "$($c.Major).$($c.Minor).$($c.Build + 1)"
if (-not $Version) {
    Write-Host ""
    Write-Host "Current version: $current"
    Write-Host "  bug fixes only       -> $suggested"
    Write-Host "  new features         -> $($c.Major).$($c.Minor + 1).0"
    Write-Host "  big breaking changes -> $($c.Major + 1).0.0"
    $Version = Read-Host "New version [$suggested]"
    if (-not $Version) { $Version = $suggested }
}
$Version = $Version.Trim().TrimStart('v')
$v = Parse-Version $Version
if (-not $v) { Stop-Release "'$Version' isn't a version like 1.2.3." }
if ($v -le $c) { Stop-Release "$Version isn't newer than $current." }
$tag = "v$Version"
if (git tag --list $tag) { Stop-Release "the tag $tag already exists here." }
if (git ls-remote --tags origin "refs/tags/$tag") { Stop-Release "the tag $tag already exists on GitHub." }

Write-Host ""
Write-Host "About to release $tag from '$branch':"
Write-Host "  1. set the version in Cargo.toml to $Version and commit it"
Write-Host "  2. tag that commit $tag"
if ($DryRun) { Write-Host "  3. (dry run: nothing is pushed)" } else { Write-Host "  3. push both to GitHub, which builds and publishes the release" }
if (-not (Ask-Yes 'Go ahead?')) { Stop-Release 'nothing was changed.' }

# 3. Bump, commit, tag.
$updated = [regex]::Replace($manifest, '(?ms)(^\[workspace\.package\]\s*\r?\nversion = ")\d+\.\d+\.\d+(")', "`${1}$Version`${2}", 1)
[System.IO.File]::WriteAllText((Join-Path $root 'Cargo.toml'), $updated)
Write-Host 'Updating Cargo.lock...'
cargo update --workspace --quiet
if ($LASTEXITCODE -ne 0) {
    git checkout -- Cargo.toml Cargo.lock
    Stop-Release 'cargo update failed; Cargo.toml was put back.'
}
git commit --quiet -am "Release $tag"
git tag -a $tag -m "Reign $tag"
Write-Host "Committed and tagged $tag."

if ($DryRun) {
    Write-Host ""
    Write-Host "Dry run: not pushed. To undo: git tag -d $tag; git reset --hard HEAD~1" -ForegroundColor Yellow
    exit 0
}

# 4. Push: the branch first, then the tag that starts the release.
git push origin HEAD
if ($LASTEXITCODE -ne 0) {
    Stop-Release "pushing failed (do you have write access to the repository?). Your commit and tag are kept: fix it, then run: git push origin HEAD $tag"
}
git push origin $tag
if ($LASTEXITCODE -ne 0) { Stop-Release "pushing the tag failed. Retry with: git push origin $tag" }

$remote = (git remote get-url origin) -replace '\.git$', '' -replace '^git@github\.com:', 'https://github.com/'
Write-Host ""
Write-Host "Pushed $tag. GitHub is now testing, building and publishing it (about 15-25 minutes)." -ForegroundColor Green
Write-Host "Watch it:      $remote/actions"
Write-Host "When it's done: $remote/releases/tag/$tag"
