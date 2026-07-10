# Brickout Revenge source assets

These are the authoritative source masters used by the remake. The running game
does not read them; committed derivatives live under `../original/`.

- `TowerDefense.swf`: final published game recovered from RootbeerGames.
- `PTDanim01-2.fla` and `PTDanim01-2.swf`: original animation project/output.
- `PTDsource.ai`: complete 660x550 vector game composition.
- `background.ai`: vector background and UI shell.
- `paddledef_02-0.ai`: vector paddle/UI working file.

The Illustrator files contain PDF 1.5-compatible vector data. Regenerate the
4x archival renders and animation frames with:

```powershell
python tools/extract_original_assets.py `
  --ffdec C:\path\to\ffdec-cli.exe `
  --pdftoppm C:\path\to\pdftoppm.exe `
  --scale 4
```

FFDec 26.2.1 was used for the initial recovery. Generated files are checked in
so desktop and Android builds do not require these tools or any external path.
