-- プラグインをロードした後、Hyprland の設定ファイルから読み込む。
local hg = assert(
    hl.plugin.hyprglass,
    "BKSS requires Hyprglass; load a compatible plugin before this configuration"
)

-- 他の window / layer に意図せず適用しないよう、対象を登録してから有効にする。
hg.config({
    enabled = false,
    layers = { enabled = false },
    subsurfaces = { enabled = false },
})

hg.layer("bkss-bottom-bar", {
    preset = "pomme",
    mask_mode = "contour",
    mask_threshold = 0.1,
    live_resample = true,
})

hg.config({ layers = { enabled = true } })
