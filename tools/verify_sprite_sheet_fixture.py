"""Verify the deterministic 2x2 sprite-sheet fixture without renderer dependencies."""
from pathlib import Path
import struct, zlib

def read_rgba(path: Path):
    data=path.read_bytes(); assert data[:8] == b"\x89PNG\r\n\x1a\n"
    pos=8; width=height=None; raw=b""
    while pos < len(data):
        n=struct.unpack(">I", data[pos:pos+4])[0]; typ=data[pos+4:pos+8]; payload=data[pos+8:pos+8+n]; pos += 12+n
        if typ == b"IHDR": width,height,depth,color=struct.unpack(">IIBB",payload[:10]); assert (depth,color)==(8,6)
        elif typ == b"IDAT": raw += payload
        elif typ == b"IEND": break
    decoded=zlib.decompress(raw); stride=width*4; rows=[]; i=0; prev=bytearray(stride)
    for _ in range(height):
        filt=decoded[i]; i += 1; cur=bytearray(decoded[i:i+stride]); i += stride
        for x in range(stride):
            left=cur[x-4] if x >= 4 else 0; up=prev[x]; ul=prev[x-4] if x >= 4 else 0
            if filt == 1: cur[x]=(cur[x]+left)&255
            elif filt == 2: cur[x]=(cur[x]+up)&255
            elif filt == 3: cur[x]=(cur[x]+((left+up)//2))&255
            elif filt == 4:
                p0=left+up-ul; pa=abs(p0-left); pb=abs(p0-up); pc=abs(p0-ul); cur[x]=(cur[x]+(left if pa<=pb and pa<=pc else up if pb<=pc else ul))&255
            elif filt != 0: raise AssertionError(filt)
        rows.append(cur); prev=cur
    return width,height,[tuple(rows[y][x:x+4]) for y in range(height) for x in range(0,stride,4)]

if __name__ == "__main__":
    path=Path(__file__).parents[1]/"tests/fixtures/sprite_sheet_2x2.png"
    assert read_rgba(path) == (2,2,[(255,0,0,255),(0,255,0,255),(0,0,255,255),(255,255,0,255)])
    print("sprite_sheet_2x2: ok")
