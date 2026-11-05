// Which OS the app runs on, for wording and key labels. The native layer does the real
// platform work; the UI only needs to say "Keychain" vs "Credential Manager" and show ⌃⌥
// instead of Ctrl+Alt on a Mac.

function detectMac(): boolean {
  if (typeof navigator === "undefined") return false;
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  const platform = nav.userAgentData?.platform || nav.platform || nav.userAgent || "";
  return /mac/i.test(platform);
}

export const IS_MAC = detectMac();

/** "macOS" or "Windows", for sentences like "macOS can't register this combination". */
export const OS_NAME = IS_MAC ? "macOS" : "Windows";

/** Where API keys are stored. */
export const KEY_STORE = IS_MAC ? "your macOS Keychain" : "Windows Credential Manager";
/** Short form for status lines ("Saved in Keychain"). */
export const KEY_STORE_SHORT = IS_MAC ? "Keychain" : "Credential Manager";

/** Where the user turns microphone access back on. */
export const MIC_PRIVACY_PLACE = IS_MAC
  ? "System Settings > Privacy & Security > Microphone"
  : "Windows Settings > Privacy > Microphone";
