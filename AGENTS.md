# AGENTS.md — Mandatory engineering rules for SZKura

> **Audience:** AI coding agents, automation, and human contributors.
>
> **Status:** Repository-wide implementation contract. Read this file **before** editing code, introducing a dependency, creating a UI, or opening a PR. It applies to all feature branches.
>
> Spanish companion: [docs/AGENTS.es.md](docs/AGENTS.es.md). The detailed UI specification is [docs/UI_DESIGN_GUIDE.md](docs/UI_DESIGN_GUIDE.md). **Read and follow both** for any visual change. If these documents disagree, preserve existing behavior, raise the discrepancy in the PR, and request a decision; do not invent a new design system.

## 1. Project context and authoritative references

SZKura is a **local-first, cross-platform Rust desktop application** for managing BOOTH/VRChat assets. It uses **egui/eframe**, **SQLite**, local managed storage, and Spanish/English translations. New work must fit the existing app rather than introduce a disconnected product.

Before implementation, inspect the relevant source and established patterns:

| Concern | Authoritative source |
| --- | --- |
| App navigation, header, product details, settings | `src/main.rs` |
| Reusable buttons, icon geometry, confirmation modals | `src/ui_support.rs` |
| Colors, palettes, shared visual tokens | `src/theme.rs` |
| All user-facing Spanish/English labels and messages | `src/i18n.rs` |
| Stable catalog semantics, SQLite preferences/migrations | `src/db.rs` |
| File linking, copying, verification | `src/managed_storage.rs`, `src/import.rs` |
| Package/archive inspection and safety | `src/package_inspect.rs` |
| Visual layout and confirmation requirements | `docs/UI_DESIGN_GUIDE.md` |
| CI and Windows/Linux test artifacts | `.github/workflows/` |

**Do not claim an established convention does not exist without inspecting these files.** Reuse existing components and data paths. Avoid unnecessary new abstractions and dependencies.

## 2. Design-system compliance is mandatory, not optional

**Do not ship a feature that works technically but looks or behaves unlike SZKura.**

- Use `theme::colors()` for every surface, border, text color, selection state, primary action, and card. Support **all four palettes** (Onyx, Jade, Amethyst, Amber); never hard-code the original green theme.
- Reuse `ui_support::action`, `icon_action`, `nav_choice`, and other established controls where appropriate. Regular buttons and inputs should follow the **36 pt** interaction height and **8 pt** button radius. Do not pad labels with spaces or emoji to simulate icons.
- Match the existing typography, spacing, and widths. Relevant guidance: page titles **23 pt**, section headings **17–18 pt**, confirmation headings **19 pt**, descriptions **13 pt**, and secondary metadata **11–12.5 pt**.
- Preserve the **single main CentralPanel** pattern for the library, settings, new-product form, and product editor. Primary back/navigation actions stay in the page header, outside scroll containers, with **one obvious way back**.
- Use the established responsive breakpoints and column layouts in `docs/UI_DESIGN_GUIDE.md`. Verify at the minimum viewport (**980×650**), at normal/wide widths, and at different Windows display scales. Buttons must never disappear behind scroll areas or overlap long translated labels.
- A screenshot or visual check is required for new screens or changed dialogs; inspect **both languages and all themes**. Compilation alone is not visual validation.

### Dialogs and workflows

- For **yes/no confirmations and consent**, use **`ui_support::confirmation_dialog(...)`**, `dialog_subject`, and `dialog_text`. It uses the established **`egui::Modal`** styling. **Do not create a generic `egui::Window` for a confirmation**, including third-party downloads and consent, without documenting and approving an exception.
- Preserve the standard modal behavior: centered, content-sized, themed, maximum approximately **440 pt** width; no duplicate title bar, no arbitrary fixed height; **Cancel** is visible, Escape cancels, and the affirmative action is on the right. The final action must be disabled while an incompatible job is running.
- For multi-step workflows (such as viewing and importing a remote asset), either use the established main-panel navigation model or a **reusable, themed workflow component** consistent with the guide. Do not introduce a standalone floating window simply because it is quick to code.
- Never communicate completion until the underlying action has actually succeeded.

## 3. Feature discoverability, navigation, and contextual actions

**Context matters as much as functionality.** Put an action where the user understands what it acts on.

- Product-specific actions must live **in the product detail header or an obvious nearby section**, not be buried among unrelated file operations. In particular, **share the current product record from its product detail**, and place **Copy share link** prominently near the top of that product detail when a link is active. Users should not need to search a floating dialog for the link.
- Global actions belong in a shared top-level navigation surface only if they are globally useful and available. Do **not show an entry point for an optional feature that is disabled in Settings**. Specifically, **Import from SZKura link** should not appear while optional sharing/transfer is disabled.
- Optional feature flags must control *both* discoverability and execution. Do not merely hide a button while leaving an alternative route that performs the restricted action. When an option is turned off, close/revoke related active sessions safely.
- Never add a new top-bar button without checking existing ordering, spacing, translations, and minimum-width behavior.
- Every workflow needs clear **Back/Cancel/Close**, loading, empty, success, and error states, including when the user disables the feature mid-operation.

## 4. Internationalization is part of the feature

SZKura supports **Spanish (`es`) and English (`en`)**. A change is **incomplete** if either language shows untranslated UI.

- The canonical UI source strings are Spanish; wrap user-facing strings with **`i18n::tr(...)`** (or the existing local `tr` import) and add the English mappings to **`src/i18n.rs`** in the same commit.
- For dynamic/status text, use the existing **`i18n::tr_message(...)`** / message-translation conventions or explicit translated templates. Never rely on English users understanding untranslated Spanish success/error messages.
- Translate **all** user-visible text: navigation, action labels, tooltips, configuration, dialogs/consent, placeholder hints, statuses, empty/error/loading states, and text containing dynamic values.
- **Do not translate** stable SQLite category identifiers, paths, filenames, URLs, product names, imported text, protocol fields, or persisted IDs. Convert identifiers to localized display labels only at the UI boundary.
- Add tests in `src/i18n.rs` covering representative strings in both languages, especially new entry points, consent dialogs, and error messages. Inspect text wrapping in English and with Japanese product titles.

**Before PR approval:** switch between Spanish and English and navigate the complete new flow; no literal Spanish text may remain in the English UI.

## 5. SQLite compatibility and existing user data

- Treat the existing catalog as irreplaceable user data. Never overwrite, delete, reclassify, or move records/files silently. Respect existing BOOTH associations and local metadata.
- When adding a persisted preference, update **both** the reader and writer allowlists in `src/db.rs`, including `Catalog::preference` and `Catalog::update_preference`. Add tests for **default value, update, persistence after reopen, and rejection of unknown keys**. A missing allowlist entry previously prevented SZKura from opening.
- Preserve stored category IDs, palette IDs, and language IDs. If a schema change is needed, make it backward-compatible and idempotent; test against a previously populated database.
- Do not put migration side effects in UI frame rendering or network callbacks. Keep disk/database work off the egui paint path.
- Prevent accidental changes to the user's library when transferring data between installations. Preview remote metadata and files **before** import; never silently overwrite a pre-existing product.

## 6. Security, optional dependencies, and transfer features

- SZKura remains usable **offline and without third-party accounts**. Additional services and executables must be optional unless explicitly approved.
- **No silent downloads** of third-party executables at installation, startup, after updates, or merely when enabling an option. Ask for explicit, informed consent **before** the first download and before replacement/updates. Cancel must leave the core application fully functional.
- If downloading a binary, use the official source, pin/validate authenticity and SHA-256, cache it without administrator privileges, and offer removal from Settings. Reuse an already verified copy.
- Stop and revoke tunnels when sharing ends, expires, or is disabled. Do not expose arbitrary files or local directories. Use secure random access tokens, scoped file selection, streaming, size limits, URL/path validation, and integrity checks.
- Temporary peer transfers are **SZKura-client-only at the ordinary browser UX level**: no HTTP browser landing page, listing, or unauthenticated file/manifest endpoint. Use a separate random per-session access key carried in the share-link URL fragment and explicitly sent by the receiving app as `X-SZKura-Transfer-Key`; a plain browser navigation must return 404. Require the header on **every** endpoint, including file/cover routes, and test this with in-process HTTP requests. Keep the source-of-truth protocol documented in `docs/SHARING.md`. **Never claim a custom header proves client identity**: a user who knows the full link can imitate the request with another HTTP tool.
- Importing archives must **never automatically execute** content. Keep existing ZIP/7z/RAR/UnityPackage safety limits and managed-storage semantics.
- Do not imply that buying a BOOTH asset permits redistribution. Remind users to respect each creator's license.
- Do not print credentials, share tokens, private paths, or personal library metadata in ordinary logs, PRs, screenshots, or test fixtures.

## 7. Implementation and verification procedure

1. **Read** `AGENTS.md`, `docs/UI_DESIGN_GUIDE.md`, and the relevant modules and tests.
2. **Describe** the user flow, affected screen, optional/disabled behavior, data changes, and translation requirements before coding.
3. **Implement** using the project's reusable components. Keep UI, network operations, database writes, and file handling separated; do not block the egui thread.
4. **Test** new logic, plus regressions across startup, preference persistence, translations, and relevant error/empty states.
5. **Verify** `cargo test --all-targets` and `cargo build --release` on Windows and Linux via the project's CI. Run `cargo clippy --all-targets` where possible. Fix failures; do not claim checks passed until results actually confirm it.
6. **Inspect UI behavior** in Spanish and English, four themes, narrow/wide windows, with long labels and Japanese titles. Verify consent, cancellation, keyboard behavior, disabled-state visibility, and correct product context.
7. **Use a feature branch and PR**. Link issues, summarize design decisions, list test results and limitations, and include manual test instructions. Do not merge to `main` without authorization.
8. **For test builds**, supply the Windows `.exe` from **the exact PR commit's successful CI artifact**, not a prior build or the `main` release. Include its source run, commit, and any remaining manual-test uncertainty.

## 8. PR acceptance checklist (copy into each feature PR)

- [ ] Existing conventions and `docs/UI_DESIGN_GUIDE.md` reviewed.
- [ ] New controls use SZKura's theme, reusable components, spacing, typography, and responsive layout.
- [ ] Dialogs/consent match the shared modal pattern; Cancel/Escape and busy states work.
- [ ] Product actions are located in the product context; global actions are discoverable only when allowed.
- [ ] Optional-feature UI is hidden and backend behavior disabled when turned off.
- [ ] **All UI strings are translated in Spanish and English**; i18n tests updated.
- [ ] Existing database, preference allowlists, backups, and managed files are preserved.
- [ ] User consent, third-party binaries, transfer security, and creator licenses reviewed where applicable.
- [ ] Rust tests, release builds, and CI checks passed on Windows/Linux; any gaps disclosed.
- [ ] Visual/manual UX verification completed or explicitly marked outstanding.
- [ ] PR screenshots, testing instructions, and links to the **correct** CI artifact added.

**If a checkbox fails, the feature is not ready to merge.** A compiling executable is not sufficient.

## 9. Updating these rules

When establishing a new UI or engineering pattern, update **both** the detailed implementation guide and this file in the same PR. Keep the Spanish companion aligned. Any deliberate exception must be explained in the PR with a reason and a regression/verification plan. Never silently bypass these rules because implementation is automated.
