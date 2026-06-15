"""Cut avatar_ai.jpeg into a circular avatar at SIZE x SIZE.

- Resize source to SIZE with Lanczos.
- Build a perfect alpha mask: inside the inscribed circle = 255,
  outside = 0. Edge anti-aliasing via high-resolution supersample.
- Composite the image onto transparent using the mask.
"""

from PIL import Image, ImageDraw

SRC = "avatar_ai.jpeg"
OUT = "avatar.png"
SIZE = 300
SUPERSAMPLE = 4


def circular_mask(size: int, supersample: int) -> Image.Image:
    big = size * supersample
    mask = Image.new("L", (big, big), 0)
    draw = ImageDraw.Draw(mask)
    draw.ellipse((0, 0, big - 1, big - 1), fill=255)
    return mask.resize((size, size), Image.Resampling.LANCZOS)


def main() -> None:
    image = Image.open(SRC).convert("RGB")
    print(f"source: {image.size} {image.mode}")

    resized = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    rgba = resized.convert("RGBA")

    mask = circular_mask(SIZE, SUPERSAMPLE)

    out = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    out.paste(rgba, mask=mask)
    out.save(OUT, format="PNG", optimize=True)

    print(f"wrote {OUT}  size={out.size}  mode={out.mode}")
    for (x, y) in [(0, 0), (SIZE - 1, 0), (0, SIZE - 1), (SIZE - 1, SIZE - 1),
                   (SIZE // 2, SIZE // 2), (SIZE // 2, 10), (10, SIZE // 2)]:
        print(f"  ({x:3},{y:3}) = {out.getpixel((x, y))}")


if __name__ == "__main__":
    main()
