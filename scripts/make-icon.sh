#!/usr/bin/env bash
set -euo pipefail

# Build assets/icon.icns from assets/icon.png (1024x1024 RGBA).
#
# Implementation notes:
#  - sips is not used. sips treats RGB as straight (non-premultiplied)
#    alpha, so RGB on alpha=0 pixels bleeds into the downsample average
#    and produces visible white halos at small sizes.
#  - iconutil is not used either. iconutil re-encodes 16x16 and 32x32
#    as the legacy is32/il32 + s8mk/l8mk pair, which round-trips RGB
#    through a separate alpha mask and re-introduces bright pixels at
#    boundary alpha values. We pack a PNG-only icns directly.
#  - The source is full-bleed artwork shared with Windows and Linux. macOS
#    before 26 draws the icns bitmap unmasked, so this script applies Apple's
#    app icon template: an 824px rounded-rect body centered on a transparent
#    1024px canvas. Transparent pixels keep RGB=0 for the alpha-weighted
#    downsample below.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT_DIR/assets/icon.png"
OUT="$ROOT_DIR/assets/icon.icns"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

if [ ! -f "$SRC" ]; then
    echo "error: $SRC not found" >&2
    exit 1
fi

read -r WIDTH HEIGHT < <(sips -g pixelWidth -g pixelHeight "$SRC" \
    | awk '/pixelWidth:/ {w=$2} /pixelHeight:/ {h=$2} END {print w, h}')
if [ "$WIDTH" != "1024" ] || [ "$HEIGHT" != "1024" ]; then
    echo "error: $SRC must be 1024x1024 (got ${WIDTH}x${HEIGHT})" >&2
    exit 1
fi

# Resample the artwork to the template body size. sips is safe here because
# the body is opaque; the straight-alpha halo problem only affects alpha=0 edges.
BODY="$TMP_DIR/body.png"
sips -s format png -z 824 824 "$SRC" --out "$BODY" >/dev/null

python3 -I - "$BODY" "$OUT" <<'PY'
import os, struct, sys, zlib

body_src, out_icns = sys.argv[1], sys.argv[2]

# Apple macOS app icon template (1024px canvas).
CANVAS = 1024
BODY = 824
INSET = (CANVAS - BODY) // 2
RADIUS = 185.4
SUPERSAMPLE = 4

def paeth(a,b,c):
    p=a+b-c; pa,pb,pc=abs(p-a),abs(p-b),abs(p-c)
    if pa<=pb and pa<=pc: return a
    if pb<=pc: return b
    return c

def decode(path):
    data=open(path,'rb').read(); i=8; idat=b""
    while i<len(data):
        L=struct.unpack(">I",data[i:i+4])[0]; i+=4
        t=data[i:i+4].decode(); i+=4
        body=data[i:i+L]; i+=L; i+=4
        if t=="IHDR":
            w,h,depth,color,_,_,interlace=struct.unpack(">IIBBBBB",body)
            assert (depth,color,interlace)==(8,6,0), \
                f"{path}: expected 8-bit non-interlaced RGBA PNG"
        elif t=="IDAT": idat+=body
        elif t=="IEND": break
    raw=zlib.decompress(idat); stride=w*4
    out=bytearray(stride*h); prev=bytearray(stride); pos=0
    for y in range(h):
        f=raw[pos]; pos+=1
        row=bytearray(raw[pos:pos+stride]); pos+=stride
        if f==1:
            for x in range(4,stride): row[x]=(row[x]+row[x-4])&0xFF
        elif f==2:
            for x in range(stride): row[x]=(row[x]+prev[x])&0xFF
        elif f==3:
            for x in range(stride):
                l=row[x-4] if x>=4 else 0
                row[x]=(row[x]+((l+prev[x])>>1))&0xFF
        elif f==4:
            for x in range(stride):
                l=row[x-4] if x>=4 else 0
                u=prev[x]; ul=prev[x-4] if x>=4 else 0
                row[x]=(row[x]+paeth(l,u,ul))&0xFF
        out[y*stride:(y+1)*stride]=row; prev=row
    return w,h,out

def encode_png_bytes(w,h,rgba):
    sig=b"\x89PNG\r\n\x1a\n"
    ihdr=struct.pack(">IIBBBBB",w,h,8,6,0,0,0)
    stride=w*4
    raw=bytearray()
    for y in range(h):
        raw.append(0); raw.extend(rgba[y*stride:(y+1)*stride])
    idat=zlib.compress(bytes(raw),9)
    def chunk(t,d):
        return struct.pack(">I",len(d))+t+d+struct.pack(">I",zlib.crc32(t+d))
    return sig + chunk(b"IHDR",ihdr) + chunk(b"IDAT",idat) + chunk(b"IEND",b"")

def downsample(w, h, rgba, factor):
    """Alpha-weighted box downsample by integer factor.
       Output pixel: alpha = mean(alpha), RGB = sum(RGB*a)/sum(a)."""
    nw, nh = w // factor, h // factor
    out = bytearray(nw * nh * 4)
    for ny in range(nh):
        y0 = ny*factor
        for nx in range(nw):
            x0 = nx*factor
            sumR=sumG=sumB=sumA=0
            for dy in range(factor):
                row_off = (y0+dy)*w*4
                for dx in range(factor):
                    o = row_off + (x0+dx)*4
                    a = rgba[o+3]
                    sumA += a
                    if a:
                        sumR += rgba[o]*a
                        sumG += rgba[o+1]*a
                        sumB += rgba[o+2]*a
            n = factor*factor
            no = (ny*nw + nx)*4
            out[no+3] = sumA // n
            if sumA:
                out[no]   = min(255, sumR // sumA)
                out[no+1] = min(255, sumG // sumA)
                out[no+2] = min(255, sumB // sumA)
            # else: stays zero (transparent)
    return nw, nh, out

def body_coverage(x, y):
    """Fraction of body pixel (x, y) inside the rounded rect, supersampled
       only near the corners where the edge is curved."""
    def inside(px, py):
        cx = min(max(px, RADIUS), BODY - RADIUS)
        cy = min(max(py, RADIUS), BODY - RADIUS)
        return (px-cx)**2 + (py-cy)**2 <= RADIUS*RADIUS
    near_x = x < RADIUS or x + 1 > BODY - RADIUS
    near_y = y < RADIUS or y + 1 > BODY - RADIUS
    if not (near_x and near_y):
        return 1.0
    hits = 0
    for sy in range(SUPERSAMPLE):
        for sx in range(SUPERSAMPLE):
            hits += inside(x + (sx+0.5)/SUPERSAMPLE, y + (sy+0.5)/SUPERSAMPLE)
    return hits / (SUPERSAMPLE*SUPERSAMPLE)

def apply_template(body):
    rgba = bytearray(CANVAS * CANVAS * 4)
    for y in range(BODY):
        for x in range(BODY):
            cov = body_coverage(x, y)
            if cov == 0:
                continue
            si = (y*BODY + x)*4
            di = ((y+INSET)*CANVAS + x + INSET)*4
            rgba[di:di+3] = body[si:si+3]
            rgba[di+3] = round(body[si+3] * cov)
    return rgba

bw,bh,body = decode(body_src)
assert bw==BODY and bh==BODY, f"expected {BODY}x{BODY} body, got {bw}x{bh}"
rgba = apply_template(body)

# Map iconset entry → icns PNG type code. Modern macOS reads PNG-typed
# entries for every size, so we never need is32/il32/s8mk/l8mk.
entries = [
    (16,   b"icp4"),
    (32,   b"ic11"),  # 16x16 @2x
    (32,   b"icp5"),
    (64,   b"ic12"),  # 32x32 @2x
    (128,  b"ic07"),
    (256,  b"ic13"),  # 128x128 @2x
    (256,  b"ic08"),
    (512,  b"ic14"),  # 256x256 @2x
    (512,  b"ic09"),
    (1024, b"ic10"),  # 512x512 @2x
]

# Cache downsamples so duplicate sizes (32, 256, 512) are computed once.
cache = {1024: rgba}
def get_at(size):
    if size in cache: return cache[size]
    factor = 1024 // size
    assert 1024 % size == 0, f"non-integer downsample for {size}"
    _, _, ds = downsample(1024, 1024, rgba, factor)
    cache[size] = ds
    return ds

# Build icns: header(8) + sum(entry header(8) + payload).
chunks = []
for size, type_code in entries:
    pixels = get_at(size)
    png = encode_png_bytes(size, size, pixels)
    chunks.append(type_code + struct.pack(">I", 8 + len(png)) + png)

body = b"".join(chunks)
icns = b"icns" + struct.pack(">I", 8 + len(body)) + body

with open(out_icns, "wb") as f:
    f.write(icns)
print(f"wrote {out_icns}  ({len(icns):,} bytes, {len(entries)} entries)")
PY

echo "Wrote: $OUT"
echo "Size:  $(du -h "$OUT" | cut -f1)"
