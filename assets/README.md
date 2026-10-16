# ForgeMCP brand assets

The mark is an orange flame forge nested inside code braces: `{ 🔥 }` = forge + code.

## Palette

| Role | Hex | Notes |
| --- | --- | --- |
| Navy (background) | `#011E4A` | Logo background, GitHub social preview canvas |
| Ember orange (accent) | `#F97316` | Primary accent, README/consent UI highlights |
| Flame gradient | `#FD8D39` → `#F97316` | Sampled from the mark's flame stroke |

## Files

| File | Size | Use |
| --- | --- | --- |
| `logo.png` | 1183×1183 | Master raster, full quality |
| `logo.jpg` | 1183×1183 | Original source export |
| `logo-512.png` | 512×512 | Docs, slides, large UI |
| `logo-256.png` | 256×256 | README header, app icons |
| `logo-128.png` | 128×128 | Embedded in the binary, served as `/logo.png` |
| `favicon-32.png` | 32×32 | Browser tab icon, served as `/favicon.png` and `/favicon.ico` |
| `apple-touch-icon.png` | 180×180 | iOS home-screen icon |
| `social-preview.png` | 1280×640 | GitHub repository social preview |

The HTTP transport embeds `logo-128.png` and `favicon-32.png` at compile time, so the
OAuth consent page is branded with no extra runtime files. To regenerate the derived
assets from a new master:

```bash
sips -s format png -Z 512 logo.jpg --out logo-512.png
sips -s format png -Z 256 logo.jpg --out logo-256.png
sips -s format png -Z 128 logo.jpg --out logo-128.png
sips -s format png -Z 32  logo.jpg --out favicon-32.png
sips -s format png -Z 180 logo.jpg --out apple-touch-icon.png
```

`social-preview.png` is the 1280×640 GitHub social preview: the mark scaled to 460 px,
centred on the navy canvas with feathered edges so no seam shows.

## GitHub repository settings

Social preview is a web-UI-only setting (there is no REST endpoint for it):

1. Repository → **Settings** → **General** → **Social preview** → **Edit** → **Upload an image…**
2. Upload `assets/social-preview.png`.
3. Also set a square avatar/logo in the organization or repository profile if desired
   (`logo-512.png`).
