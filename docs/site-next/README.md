# Website for the next release

`index.html` here is the landing page as it should read once the next release (the one `CHANGELOG.md` calls
"Unreleased") is published. It lives outside `site/` on purpose: `.github/workflows/pages.yml` deploys the whole
`site/` folder, so anything put there goes public and is indexed at once, while the download buttons still fetch
the current release.

Only `index.html` differs from `site/`. `style.css`, `main.js`, the images and the fonts are unchanged (the new
section and list items use classes the page already has).

## When the release is published

1. Fill in the three placeholders in the first entry of "What's new" (search for `@@`): `@@VERSION@@`, `@@ISO_DATE@@`
   (2026-MM-DD, for the `datetime` attribute) and `@@DATE@@` (for example `20 Nov 2026`), and delete the `@@RELEASE`
   comment. Make the text match the final "What's new" lead items in `CHANGELOG.md`.
2. Copy `docs/site-next/index.html` over `site/index.html`.
3. Check nothing is left: `grep -n "@@" site/index.html` must print nothing.
4. Commit and push; the Website workflow deploys it.

Screenshots: nothing new is referenced. See the report that came with this folder for the screens worth taking; add
them to `site/assets/` and then give each `<img>` a `?v=` hash like the others.
