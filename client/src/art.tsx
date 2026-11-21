import { useId } from "react";
import { imageUrl } from "./api";

function hash(s: string, salt = 0): number {
  let h = 2166136261 ^ salt;
  for (const c of s) {
    h ^= c.charCodeAt(0);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** Placeholder cover art until games upload their own: graphite with a faint monochrome
 *  pattern picked from the slug, so games are distinguishable without loud colour. */
export function GameArt({ slug, className }: { slug: string; className?: string }) {
  const id = useId().replace(/:/g, "");
  const kind = hash(slug, 2) % 4;
  const r = (n: number) => (hash(slug, n) % 1000) / 1000;
  const light = 15 + Math.round(r(1) * 8); // 15-23% lightness

  let pattern;
  if (kind === 0) {
    const cx = r(3) > 0.5 ? 300 : 0;
    const cy = r(4) > 0.5 ? 300 : 0;
    pattern = Array.from({ length: 9 }, (_, i) => (
      <circle key={i} cx={cx} cy={cy} r={40 + i * 34} fill="none" stroke="white" strokeOpacity={0.04 + (i % 3) * 0.015} strokeWidth={i % 2 ? 10 : 3} />
    ));
  } else if (kind === 1) {
    pattern = (
      <g transform={`rotate(${r(5) > 0.5 ? 35 : -35} 150 150)`}>
        {Array.from({ length: 16 }, (_, i) => (
          <rect key={i} x={-150 + i * 40} y={-150} width={i % 3 === 0 ? 18 : 6} height={600} fill="white" fillOpacity={0.035 + (i % 4) * 0.01} />
        ))}
      </g>
    );
  } else if (kind === 2) {
    pattern = Array.from({ length: 144 }, (_, i) => {
      const x = (i % 12) * 26 + 6;
      const y = Math.floor(i / 12) * 26 + 6;
      return <circle key={i} cx={x} cy={y} r={1.5 + ((x + y) / 600) * 4} fill="white" fillOpacity={0.03 + (y / 300) * 0.09} />;
    });
  } else {
    pattern = [0, 1, 2].map((i) => (
      <circle key={i} cx={r(6 + i) * 300} cy={r(9 + i) * 300} r={60 + r(12 + i) * 90} fill="white" fillOpacity={0.05} />
    ));
  }

  return (
    <svg className={className} viewBox="0 0 300 300" preserveAspectRatio="xMidYMid slice" style={{ display: "block", width: "100%", height: "100%" }}>
      <defs>
        <linearGradient id={`g${id}`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor={`hsl(220 6% ${light + 8}%)`} />
          <stop offset="1" stopColor={`hsl(220 8% ${light - 9}%)`} />
        </linearGradient>
      </defs>
      <rect width="300" height="300" fill={`url(#g${id})`} />
      {pattern}
    </svg>
  );
}

/** A game's uploaded poster or banner, falling back to generated art. */
export function GameImage({ game, kind }: { game: { slug: string; cover?: string | null; banner?: string | null }; kind: "cover" | "banner" }) {
  // Banners fall back to the cover (cropped) before the placeholder, and vice versa.
  const id = kind === "cover" ? (game.cover ?? game.banner) : (game.banner ?? game.cover);
  const url = imageUrl(id);
  if (!url) return <GameArt slug={game.slug} />;
  return <img src={url} alt="" draggable={false} style={{ display: "block", width: "100%", height: "100%", objectFit: "cover" }} />;
}
