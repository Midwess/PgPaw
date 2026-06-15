"""Cut avatar_ai.jpeg into a circular avatar at SIZE x SIZE.

- Find the bbox of the non-cream content (the animals) and crop to it
  with a small cream padding. The animals then fill most of the icon
  instead of swimming in a wide cream halo.
- Resize the crop to SIZE with Lanczos.
- Build a perfect alpha mask: inside the inscribed circle = 255,
  outside = 0. Edge anti-aliasing via high-resolution supersample.
- Composite the image onto transparent using the mask.
"""

from PIL import Image, ImageDraw

SRC = "avatar_ai.jpeg"
OUT = "avatar.png"
SIZE = 300
SUPERSAMPLE = 4
CREAM_PADDING = 24


def content_bbox(image: Image.Image) -> tuple[int, int, int, int]:
    """Bounding box of pixels that are not the cream disc.

    Cream pixels: chroma ~30, avg ~235. Animals: chroma > 50 or avg < 200.
    The threshold chroma > 40 / avg < 180 sits well clear of JPEG
    compression noise on the cream (which tops out around chroma 31).
    """
    rgb = image.convert("RGB")
    pixels = rgb.load()
    width, height = rgb.size
    min_x, min_y = width, height
    max_x, max_y = -1, -1
    for y in range(height):
        for x in range(width):
            r, g, b = pixels[x, y]
            chroma = max(r, g, b) - min(r, g, b)
            avg = (r + g + b) / 3
            if chroma > 40 or avg < 180:
                if x < min_x:
                    min_x = x
                if x > max_x:
                    max_x = x
                if y < min_y:
                    min_y = y
                if y > max_y:
                    max_y = y
    return min_x, min_y, max_x, max_y


def circular_mask(size: int, supersample: int) -> Image.Image:
    big = size * supersample
    mask = Image.new("L", (big, big), 0)
    draw = ImageDraw.Draw(mask)
    draw.ellipse((0, 0, big - 1, big - 1), fill=255)
    return mask.resize((size, size), Image.Resampling.LANCZOS)


def main() -> None:
    image = Image.open(SRC).convert("RGB")
    print(f"source: {image.size} {image.mode}")

    bbox = content_bbox(image)
    min_x, min_y, max_x, max_y = bbox
    crop_box = (
        max(0, min_x - CREAM_PADDING),
        max(0, min_y - CREAM_PADDING),
        min(image.size[0], max_x + 1 + CREAM_PADDING),
        min(image.size[1], max_y + 1 + CREAM_PADDING),
    )
    print(f"content bbox: {bbox}  crop: {crop_box}")
    cropped = image.crop(crop_box)

    resized = cropped.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
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
