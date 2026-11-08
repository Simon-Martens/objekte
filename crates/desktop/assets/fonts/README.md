# Bundled desktop fonts

The desktop binary embeds these static TrueType fonts and registers them with GPUI before opening its first window. Runtime font downloads and host font installation are unnecessary. GPUI's platform text system supplies fallback fonts for glyphs outside the bundled families.

| Family | Official source | Pinned commit | Files |
| --- | --- | --- | --- |
| Source Sans 3 | https://github.com/adobe-fonts/source-sans | `87b37a2daaed80fcb8e8ccb0085c4d72ddade12e` (`release`) | Regular, Semibold, Bold |
| Source Code Pro | https://github.com/adobe-fonts/source-code-pro | `803b7e23ec97ae58b6232ea76519a76d428ba268` (`release`) | Regular, Bold |

Both distributions use the SIL Open Font License 1.1. Their original copyright statements and full license texts are preserved in their respective `LICENSE.md` files. No separate `NOTICE` file was present at either pinned commit; the copyright notice is in each license file.

SHA-256 of the bundled files:

| File | SHA-256 |
| --- | --- |
| `source-sans-3/SourceSans3-Regular.ttf` | `4644c81b86ec9caaa76b634889968ed3c4f4f52f054855933acc7c2b21e53b0f` |
| `source-sans-3/SourceSans3-Semibold.ttf` | `a3f4f8dcf343a8f24dc61951de93f3ba1558b15cd250ba24af8a40e957081b7d` |
| `source-sans-3/SourceSans3-Bold.ttf` | `9214b9d95e4231c609802815c2646c98174e2102d0d37f88978a7f8e71006e6a` |
| `source-code-pro/SourceCodePro-Regular.ttf` | `74bd80d3e42a08517cd7e1108ba3d86f2da29ac0f3065be95e0357956ab9db37` |
| `source-code-pro/SourceCodePro-Bold.ttf` | `b2095e0d657e6d28dc32444a9dacabab0c9241d0bf39d96371756cc9bdbc3a5f` |
