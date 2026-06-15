"""Post-process avatar_ai.jpeg (cream disc on lighter cream background)
into a 160x160 transparent PNG that keeps the cream disc as a circle.

Strategy: BFS flood-fill from the four corners, marking any pixel that
looks like the outer cream (very low chroma + bright) as transparent.
The BFS stops at the disc edge where the color shifts to the darker
disc cream, naturally producing a clean circular mask. White highlights
on the animals are bounded by saturated blue/orange so the BFS can't
reach them.
"""

from collections import deque
from PIL import Image

SRC = "avatar_ai.jpeg"
OUT = "avatar.png"
SIZE = 160


def is_outer_cream(rgb):
    r, g, b = rgb
    chroma = max(r, g, b) - min(r, g, b)
    avg = (r + g + b) / 3
    return chroma < 30 and avg > 230


def flood_to_transparent(rgba, width, height):
    pixels = rgba.load()
    visited = [[False] * height for _ in range(width)]
    queue: deque[tuple[int, int]] = deque()
    seeds = [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ]
    for sx, sy in seeds:
        r, g, b, _ = pixels[sx, sy]
        if is_outer_cream((r, g, b)):
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
                if is_outer_cream((nr, ng, nb)):
                    visited[nx][ny] = True
                    queue.append((nx, ny))

    return rgba


def main() -> None:
    image = Image.open(SRC).convert("RGB")
    resized = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    rgba = resized.convert("RGBA")
    w, h = rgba.size
    print(f"source size: {image.size}  output size: {w}x{h}")

    transparent = flood_to_transparent(rgba, w, h)
    transparent.save(OUT, format="PNG", optimize=True)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
