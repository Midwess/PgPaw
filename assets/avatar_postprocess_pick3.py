"""Post-process pick3_1.jpeg (cream disc) into a 120x120 transparent PNG.

Source is an AI-generated mascot on a cream disc on a white background.
Standard white-only flood-fill leaves the cream disc opaque. Use a chroma
criterion instead (max(R,G,B) - min(R,G,B) < 50) so both white and cream
become transparent. Saturated blue/orange animals are naturally bounded
by high chroma, so white details inside the animals (tusk, eye) stay opaque.
"""

import math
from collections import deque
from PIL import Image

SRC = "pick3_1.jpeg"
OUT = "avatar.png"
SIZE = 120
CHROMA_LIMIT = 90
LIGHT_LIMIT = 200


def is_background(r: int, g: int, b: int) -> bool:
    avg = (r + g + b) / 3
    if avg < LIGHT_LIMIT:
        return False
    return max(r, g, b) - min(r, g, b) < CHROMA_LIMIT


def find_disc_seed(pixels, width, height):
    cx, cy = width // 2, height // 2
    max_radius = min(width, height) // 2
    for radius in range(max_radius):
        for angle_step in range(16):
            angle = (angle_step / 16) * 2 * math.pi
            sx = cx + int(radius * math.cos(angle))
            sy = cy + int(radius * math.sin(angle))
            if 0 <= sx < width and 0 <= sy < height:
                r, g, b, _ = pixels[sx, sy]
                if is_background(r, g, b):
                    return sx, sy
    return None


def flood_alpha_key(image: Image.Image) -> Image.Image:
    rgba = image.convert("RGBA")
    width, height = rgba.size
    pixels = rgba.load()
    visited = [[False] * height for _ in range(width)]
    queue: deque[tuple[int, int]] = deque()

    seeds: list[tuple[int, int]] = [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ]

    disc_seed = find_disc_seed(pixels, width, height)
    if disc_seed is not None:
        seeds.append(disc_seed)

    for sx, sy in seeds:
        r, g, b, _ = pixels[sx, sy]
        if is_background(r, g, b):
            visited[sx][sy] = True
            queue.append((sx, sy))

    while queue:
        x, y = queue.popleft()
        r, g, b, _ = pixels[x, y]
        pixels[x, y] = (r, g, b, 0)
        for dx, dy in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            nx, ny = x + dx, y + dy
            if 0 <= nx < width and 0 <= ny < height and not visited[nx][ny]:
                nr, ng, nb, _ = pixels[nx, ny]
                if is_background(nr, ng, nb):
                    visited[nx][ny] = True
                    queue.append((nx, ny))

    return rgba


def main() -> None:
    image = Image.open(SRC)
    print(f"source: {image.size} {image.mode}")

    resized = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    transparent = flood_alpha_key(resized)
    transparent.save(OUT, format="PNG", optimize=True)

    out = Image.open(OUT)
    print(f"wrote {OUT}  size={out.size}  mode={out.mode}")
    for (x, y) in [(0, 0), (119, 0), (0, 119), (119, 119), (60, 60), (30, 30), (90, 30), (60, 90)]:
        print(f"  ({x:3},{y:3}) = {out.getpixel((x, y))}")


if __name__ == "__main__":
    main()
