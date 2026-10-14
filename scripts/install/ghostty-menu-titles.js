// Menu item titles Ghostty shows on this Mac, for NSUserKeyEquivalents (whose
// keys are the LOCALIZED titles). Used by components/45-ghostty-menu.sh:
//   osascript -l JavaScript ghostty-menu-titles.js <Ghostty.app> <langs> <domain>
// <langs> is a comma list overriding AppleLanguages (empty = read it from
// <domain>, then the global domain, like the app does).
// Prints "LANG\t<summary>" then one "<English title>\t<title>" line per item,
// or a single "ERROR\t<reason>" line.
//
// Localization follows CFBundle's rule, measured with a probe app: a framework
// (AppKit, which owns "Show Next/Previous Tab") speaks the MAIN bundle's
// language unless the app sets CFBundleAllowMixedLocalizations. Ghostty 1.3
// ships only its development region (en), so its menu is English even when
// pt-BR comes first in AppleLanguages. "Reload Configuration" lives in
// Ghostty's MainMenu.nib, which is English only.
ObjC.import('Foundation');

function run(argv) {
  const [app, langsArg, domain] = argv;
  const un = (o) => (o === undefined || o.isNil()) ? undefined : ObjC.deepUnwrap(o);
  const prefs = langsArg
    ? langsArg.split(',').map((s) => s.trim()).filter(Boolean)
    : un($.NSUserDefaults.alloc.initWithSuiteName(domain).arrayForKey('AppleLanguages')) || [];
  const pick = (locs, p) =>
    un($.NSBundle.preferredLocalizationsFromArrayForPreferences(locs, p.length ? p : $()))[0];

  const bundle = $.NSBundle.bundleWithPath(app);
  if (bundle.isNil()) return 'ERROR\tno app bundle at ' + app;
  const info = un(bundle.infoDictionary) || {};
  const dev = info.CFBundleDevelopmentRegion || 'en';
  const appLocs = (un(bundle.localizations) || []).filter((l) => l !== 'Base');
  const appLoc = appLocs.length ? pick(appLocs, prefs) : dev;

  const table = un($.NSDictionary.dictionaryWithContentsOfFile(
    '/System/Library/Frameworks/AppKit.framework/Resources/MenuCommands.loctable'));
  if (!table) return 'ERROR\tcannot read AppKit MenuCommands.loctable';
  const akLocs = Object.keys(table).filter((l) => l !== 'LocProvenance');
  const akLoc = info.CFBundleAllowMixedLocalizations === true
    ? pick(akLocs, prefs)
    : pick(akLocs, [appLoc]);

  const out = ['LANG\t' + akLoc + ' (AppKit), ' + appLoc + ' (Ghostty), AppleLanguages ' + (prefs.join(',') || 'unset')];
  for (const key of ['Show Next Tab', 'Show Previous Tab']) {
    const title = (table[akLoc] || {})[key];
    if (!title) return 'ERROR\tno ' + akLoc + ' title for "' + key + '" in AppKit MenuCommands.loctable';
    out.push(key + '\t' + title);
  }
  if (appLoc !== dev || !/^en/.test(dev)) {
    return 'ERROR\tGhostty menu is localized to ' + appLoc + '; its "Reload Configuration" title is unknown';
  }
  out.push('Reload Configuration\tReload Configuration');
  return out.join('\n');
}
