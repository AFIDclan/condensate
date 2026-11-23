// Minimal stroke icons (24px grid, currentColor).
const paths: Record<string, string> = {
  play: "M7 4.5v15l13-7.5z",
  download: "M12 4v11m0 0l-4.5-4.5M12 15l4.5-4.5M5 19.5h14",
  update: "M20 12a8 8 0 1 1-2.34-5.66M20 4v5h-5",
  folder: "M3.5 6.5a2 2 0 0 1 2-2h4l2 2.5h7a2 2 0 0 1 2 2v8.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z",
  trash: "M4.5 7h15M9.5 7V4.5h5V7M6.5 7l1 13h9l1-13",
  compass: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM15.5 8.5l-2 5-5 2 2-5z",
  users: "M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7zM2.5 20a6.5 6.5 0 0 1 13 0M16 4.3a3.5 3.5 0 0 1 0 6.4M18.5 14.5a6.5 6.5 0 0 1 3 5.5",
  search: "M10.5 18a7.5 7.5 0 1 0 0-15 7.5 7.5 0 0 0 0 15zM16 16l5 5",
  home: "M3.5 11L12 4l8.5 7M6 9.5V20h12V9.5",
  grid: "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
  plus: "M12 5v14M5 12h14",
  x: "M6 6l12 12M18 6L6 18",
  notes: "M6 3.5h9l3.5 3.5v13.5H6zM9 11h6M9 15h6",
  ticket: "M3.5 8a2 2 0 0 0 0 4 2 2 0 0 1 0 4V18h17v-2a2 2 0 0 1 0-4 2 2 0 0 0 0-4V6h-17zM14 6v12",
  check: "M5 12.5l4.5 4.5L19 7.5",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  chevron: "M9 5l7 7-7 7",
  mic: "M12 3.5a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0v-5a3 3 0 0 0-3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21",
  "mic-off": "M15 9.5v-3a3 3 0 0 0-5.6-1.5M9 9v2.5a3 3 0 0 0 4.7 2.5M5.5 11a6.5 6.5 0 0 0 10.8 4.9M18.5 11a6.4 6.4 0 0 1-.5 2.5M12 17.5V21M4 4l16 16",
  headphones: "M4 18v-5a8 8 0 0 1 16 0v5M4 15h3v5H5a1 1 0 0 1-1-1zM20 15h-3v5h2a1 1 0 0 0 1-1z",
  "headphones-off": "M6.3 6.3A8 8 0 0 0 4 12v6M20 18v-6a8 8 0 0 0-11.5-7.2M4 15h3v5H5a1 1 0 0 1-1-1zM20 15h-3v5h2M4 4l16 16",
};

export function Icon({ name, size = 18, className }: { name: keyof typeof paths | string; size?: number; className?: string }) {
  const filled = name === "play";
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={filled ? 0 : 2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={paths[name] ?? ""} />
    </svg>
  );
}
