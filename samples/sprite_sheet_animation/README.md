# Sprite sheet animation

This executable sample loads one 2x2 sprite sheet and draws its four row-major cells through `SpriteSheet.draw_frame`.

```powershell
stasis --workspace samples/sprite_sheet_animation check
stasis --workspace samples/sprite_sheet_animation build
```

The colored 2x2 fixture is intentionally tiny so automated tests can verify exact UV selection without image-generation noise.