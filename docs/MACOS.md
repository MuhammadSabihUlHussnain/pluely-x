# Pluely X on macOS

Pluely X runs on Macs with **macOS 14.4 (Sonoma) or later**, on both Apple Silicon and Intel
(the `.dmg` is a universal build).

> **Status: preview.** The macOS build compiles and passes the test suite on GitHub's Mac
> runners, but it hasn't been used on a real MacBook yet. Audio capture, the permission
> prompts and the overlay behaviour are untested in practice. Please report what you see.

## Install

1. Download `Pluely-X-macOS-universal` from the latest run of the
   [macOS workflow](../../../actions/workflows/macos.yml) and open the `.dmg` inside.
2. Drag **Pluely X** into **Applications**.
3. The app isn't signed with an Apple Developer ID, so the first launch needs one extra step:
   - Right-click **Pluely X** in Applications, choose **Open**, then **Open** again. Or
   - run once in Terminal: `xattr -dr com.apple.quarantine "/Applications/Pluely X.app"`

Pluely X is a menu-bar app: it has no Dock icon. Look for its icon in the menu bar.

## Permissions

macOS asks for each one the first time it's needed. If you said no, turn it on later in
**System Settings > Privacy & Security**:

| Permission | Asked when | Used for |
|---|---|---|
| **Microphone** | You start listening | Transcribing what you say |
| **Screen & System Audio Recording → System Audio Recording Only** | You start listening | Hearing the other side of the call |
| **Screen & System Audio Recording** (screen) | You ask about your screen | The screenshot you asked for |

After changing a permission, stop and start listening (or quit and reopen Pluely X).

Because the build is ad-hoc signed, macOS may ask again for these permissions and for
Keychain access ("Pluely X wants to use your confidential information") after each update.
Choose **Always Allow** for the Keychain prompt.

## How it differs from Windows

- **The other side of the call:** Pluely X uses a Core Audio *system-audio tap*. It hears
  everything any app plays, whichever speakers or headphones it plays on, so the Windows
  "call plays on another device" warning doesn't apply.
- **Echo cancellation** on the microphone isn't available yet. Your own voice is still
  de-duplicated from the transcript.
- **Shortcuts** are the same keys: <kbd>⌃ Control</kbd>+<kbd>⌥ Option</kbd> where Windows
  uses Ctrl+Alt (for example ⌃⌥L starts and stops listening). Change them in Settings.
- **API keys** are stored in your macOS Keychain.
- The overlay follows you to every desktop (Space) and shows over full-screen apps.

## Known limits / not yet verified on a Mac

- Hiding the overlay from screen sharing uses macOS's window sharing setting. Recent macOS
  versions may not honour it for every capture method; check with your call app before
  relying on it.
- If system audio stays silent, check that **System Audio Recording** is allowed for
  Pluely X, then run **Quick scan & fix** (stethoscope icon).

## Build it yourself

You need a Mac with Xcode (or the Command Line Tools), Node.js 20+ and Rust stable.

```sh
npm ci
npx tauri dev                                                       # run in development
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npx tauri build --target universal-apple-darwin --bundles app,dmg   # universal .dmg
```

The `.dmg` is written to `src-tauri/target/universal-apple-darwin/release/bundle/dmg/`.
