# Releasing Reign

How a new version of Reign gets to users. You do two things: a one-time
setup, then **double-click `release.bat`** whenever you want to ship.

## One-time setup

1. **Get write access** to `AshtonJackson-LS/reign-engine` (the owner adds
   you under Settings → Collaborators).
2. **Add the release key to GitHub.** Open
   `C:\Users\cbfox\reign-release.key` in Notepad and copy the line inside.
   On GitHub, go to the repository's **Settings → Secrets and variables →
   Actions → New repository secret**. Set the name to `REIGN_RELEASE_KEY`,
   paste the line as the secret, and save.
3. **Back the key file up** somewhere safe, like a password manager. Never
   commit it or send it to anyone: whoever has it can push updates to every
   Reign user. If it's lost, see "Lost key" below.

That's all. GitHub Actions is already set up (`.github/workflows/release.yml`).

## Every release

1. Merge what you want to ship into `main`, then `git checkout main` and
   `git pull`.
2. Double-click **`release.bat`**. It asks for the new version (press Enter to
   take the suggestion) and checks with you before doing anything. Then it:
   - puts the version in `Cargo.toml`,
   - commits and tags it (for example `v0.2.0`),
   - pushes both to GitHub.
3. Wait about 20 minutes. GitHub runs every test, builds Windows, macOS and
   Linux, signs the update and publishes the release. Watch it under the
   repository's **Actions** tab.
4. Done. Users download the new version, and installed hubs show the green
   update arrow the next time they open.

### Picking the version number

| You changed… | Example | Bump |
| --- | --- | --- |
| Only bug fixes | 0.2.0 → 0.2.1 | last number |
| Added features | 0.2.1 → 0.3.0 | middle number |
| Broke old projects or scenes | 0.3.0 → 1.0.0 | first number |

## What users get

- **New users:** the download link
  `https://github.com/AshtonJackson-LS/reign-engine/releases/latest/download/ReignSetup.exe`
  always points at the newest installer. Running it installs Reign Hub to
  the Start menu and the Desktop, with an Uninstall entry in Settings → Apps.
  No administrator rights are needed.
- **Existing users:** a green arrow in the hub's top bar. One click
  downloads, checks the signature and restarts on the new version.
- **Projects** stay on the engine version they were made with until someone
  changes it in the hub (••• → Engine version…).

## If something goes wrong

| Problem | Fix |
| --- | --- |
| `release.bat` says "pushing failed" | You don't have write access yet (one-time setup, step 1). Your commit and tag are kept: once you have access, run the `git push` line it printed. |
| The Actions run fails at "The release key is set" | Add the secret (one-time setup, step 2), then on the failed run click **Re-run all jobs**. |
| The Actions run fails at "Sign" | The secret isn't the key the hub trusts. Paste the key file's line into the secret again, then re-run. |
| The Actions run fails at clippy or a test | Nothing was published. Fix it on `main`, then run `release.bat` again with the next version (for example 0.2.1). |
| Only the macOS or Linux job failed | The Windows release is already out. Fix it later; the next release brings those builds back. |
| Users see "Windows protected your PC" | Normal for an installer that isn't code-signed: **More info → Run anyway**. Code signing removes it (SignPath offers it free to open-source projects). |
| Lost key | Make a new key: `cargo run -p reign_hub --bin reign-release -- keygen C:\somewhere\new.key`. Put the printed public key in `PUBLIC_KEY` in `hub/src/signing.rs`, update the secret, and release. Hubs from before can't verify the new key, so their users reinstall once with ReignSetup.exe. |

## Testing a build without releasing

- **`update-reign.bat`** builds everything from your folder and installs it
  on this computer, the same way `ReignSetup.exe` does for users.
- **`scripts\build-setup.ps1`** only builds `dist\ReignSetup.exe`, so you can
  try the installer or send it to someone.
- **`scripts\release.ps1 -DryRun`** does a whole release locally (version,
  commit, tag) without pushing. It prints how to undo it.
