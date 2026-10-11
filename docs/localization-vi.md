# Vietnamese terminology and dynamic UI labels

This supplements the Vietnamese catalog merged on 2026-10-11. The language registry, live
language switching and the CJK font loader are unchanged; `vi.tsv` is a new catalog.
Translations use the meaning of PhotoCraft's English labels and ordinary Vietnamese computing
vocabulary. No proprietary translation resources, fonts, screenshots or user documents are
included.

## Terminology

| English source | Vietnamese UI |
|---|---|
| File | Tệp |
| Layer | Lớp |
| Selection | Vùng chọn |
| Canvas | Vùng vẽ |
| Artboard | Khung vẽ |
| Brush (tool) | Cọ vẽ |
| Blur | Làm mờ |
| Sharpen (command) | Tăng nét |
| Sharpen (menu) | Tăng nét |
| Layer Mask | Mặt nạ lớp |
| Vector Mask | Mặt nạ vectơ |
| Smart Object | Đối tượng thông minh |
| Smart Filter | Bộ lọc thông minh |
| Gradient | Chuyển màu |
| Opacity | Độ mờ |
| Fill (menu) | Tô |
| Stroke | Nét |
| Content-Aware Fill | Tô nhận biết nội dung |
| Puppet Warp | Uốn búp bê |
| Liquify | Hoà lỏng |
| Puppet/Puppet Warp dialog title | Uốn búp bê… |
| Vanishing Point | Điểm tụ |
| Eye Dropper | Bút lấy màu |
| Magnetic Lasso | Dây mềm từ tính |
| Type (menu) | Chữ |
| Character | Ký tự |
| Paragraph | Đoạn văn |
| Leading | Giãn dòng |
| Tracking | Độ giãn khoảng |
| Preferences | Tùy chọn |
| Workspace | Không gian làm việc |
| Swatches | Bảng mẫu màu |
| History (panel) | Lịch sử |
| Vibrance | Rực rỡ |
| Vivid Light | Ánh sáng rực rỡ |

Command-ID translations disambiguate menu names from actions: `select.all` reads "Chọn tất cả"
while the plain label `All` reads "Tất cả", and `filter.sharpen.sharpen` keeps the action
wording "Tăng nét" against the menu name. Technology names, units and user-provided names are not
translated: PhotoCraft, ArtCraft, OpenType, CMYK, Lab, GPU, HDR, LUT, ICC, sRGB and the `%`/`px`
units stay in Latin letters. The trailing `…` of dialog commands is kept.

## Grammatical notes

Vietnamese counts take the singular for 1 and one form for every other count, so the catalog's
plural rule is English's one/other and the noun never changes (`{n} lớp` for both
"{n} layer" and "{n} layers"). Sentence case follows the source: menu names are capitalised
only where the English label is ("Chỉnh sửa", "Lớp", "Bộ lọc"), while settings and hints keep a
lower-case first word ("mờ dần…", "bám vào lưới"). Vietnamese writes the accent marks as
precomposed characters, which is what the catalog stores.

## Dynamic coverage

The catalog translates the same dynamic labels the other complete catalogs do: generated filter
and adjustment parameter names (Camera Raw and the render filters), gallery categories and
effects, texture presets, job titles, layer-style menus, blend mode names, layer colour labels,
font-weight names (with the `fontWeight` context keeping the weight reading of "Light" apart
from the Camera Raw tonal region), Camera Raw labels (`cameraRaw`) and File Info EXIF fields
(`cameraData`). The catalogue's 3,665 rows cover the 1,669 source keys the `i18n-coverage`
universe measures plus ~160 `tl!(expr)` call sites it cannot enumerate.

## What this is not

Engine error messages, arbitrary plug-in labels and user document/layer/preset names remain
English or user text in every language, Vietnamese included; no language reaches `full` for that
reason. The catalog has not been reviewed by a native speaker yet, and `docs/localization.md`
still applies: labels fall back to English when a row is missing.

## Validation

- `cargo test -p photocraft-ui-egui`: catalog parse, placeholder, ellipsis, plural-form, menu,
  `tl!` literal, dynamic-label and Vietnamese terminology tests all pass; the live
  language-switch tests (including the offscreen render) pass.
- `cargo xtask i18n-coverage`: `vi 1669/1669 100.00%`, the same as every other shipped catalog.
- `cargo test -p xtask i18n_coverage`: the source key set is covered by the Vietnamese catalog.
- Clippy for the UI crate and all targets passes with warnings denied; `cargo xtask layers` and
  `cargo xtask wasm` pass.