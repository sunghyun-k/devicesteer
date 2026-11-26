// Icons, drawn once on a 20-unit grid and used by reference (`<Icon name="i-…">`). The sprite
// is rendered once per page (`<IconSprite />`).

export function IconSprite() {
  return (
    <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
      <symbol id="i-sidebar" viewBox="0 0 20 20"><rect x="2.75" y="4" width="14.5" height="12" rx="2.5"/><path d="M7.75 4v12M4.9 7h1M4.9 9.25h1"/></symbol>
      <symbol id="i-inspector" viewBox="0 0 20 20"><rect x="2.75" y="4" width="14.5" height="12" rx="2.5"/><path d="M12.25 4v12M14.1 7h1M14.1 9.25h1"/></symbol>
      <symbol id="i-search" viewBox="0 0 20 20"><circle cx="9" cy="9" r="5.25"/><path d="m13 13 3.5 3.5"/></symbol>
      <symbol id="i-refresh" viewBox="0 0 20 20"><path d="M16 10a6 6 0 1 1-1.9-4.4M16.25 3.75v3.5h-3.5"/></symbol>
      <symbol id="i-zoom-in" viewBox="0 0 20 20"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.3 3.3M9 6.6v4.8M6.6 9h4.8"/></symbol>
      <symbol id="i-zoom-out" viewBox="0 0 20 20"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.3 3.3M6.6 9h4.8"/></symbol>
      <symbol id="i-zoom-actual" viewBox="0 0 20 20"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.3 3.3M8.1 7.4 9.3 6.6v4.8"/></symbol>
      <symbol id="i-fit" viewBox="0 0 20 20"><path d="M3.5 7.5v-4h4M16.5 7.5v-4h-4M3.5 12.5v4h4M16.5 12.5v4h-4"/><rect x="7.25" y="6.5" width="5.5" height="7" rx="1.2"/></symbol>
      <symbol id="i-more" viewBox="0 0 20 20"><circle cx="4.75" cy="10" r=".9" fill="currentColor"/><circle cx="10" cy="10" r=".9" fill="currentColor"/><circle cx="15.25" cy="10" r=".9" fill="currentColor"/></symbol>
      <symbol id="i-sliders" viewBox="0 0 20 20"><path d="M3 6h8.5M15 6h2M3 14h2M8.5 14H17"/><circle cx="13.25" cy="6" r="1.75"/><circle cx="6.75" cy="14" r="1.75"/></symbol>
      <symbol id="i-info" viewBox="0 0 20 20"><circle cx="10" cy="10" r="7"/><path d="M10 9v4.5M10 6.4v.1"/></symbol>
      <symbol id="i-home" viewBox="0 0 20 20"><g fill="currentColor" stroke="none"><circle cx="5.5" cy="5.5" r="1.45"/><circle cx="10" cy="5.5" r="1.45"/><circle cx="14.5" cy="5.5" r="1.45"/><circle cx="5.5" cy="10" r="1.45"/><circle cx="10" cy="10" r="1.45"/><circle cx="14.5" cy="10" r="1.45"/><circle cx="5.5" cy="14.5" r="1.45"/><circle cx="10" cy="14.5" r="1.45"/><circle cx="14.5" cy="14.5" r="1.45"/></g></symbol>
      <symbol id="i-camera" viewBox="0 0 20 20"><path d="M2.75 8.25a2 2 0 0 1 2-2h1.9l1.2-1.75h4.3l1.2 1.75h1.9a2 2 0 0 1 2 2v6.25a2 2 0 0 1-2 2H4.75a2 2 0 0 1-2-2Z"/><circle cx="10" cy="11.1" r="2.85"/></symbol>
      <symbol id="i-record" viewBox="0 0 20 20"><circle cx="10" cy="10" r="5.5" fill="currentColor" stroke="none"/></symbol>
      <symbol id="i-stop" viewBox="0 0 20 20"><rect x="5.25" y="5.25" width="9.5" height="9.5" rx="1.6" fill="currentColor" stroke="none"/></symbol>
      <symbol id="i-appearance" viewBox="0 0 20 20"><circle cx="10" cy="10" r="6.75"/><path d="M10 3.25a6.75 6.75 0 0 0 0 13.5Z" fill="currentColor"/></symbol>
      <symbol id="i-glass" viewBox="0 0 20 20"><rect x="2.75" y="4.75" width="10.5" height="8" rx="2.5"/><rect x="6.75" y="7.25" width="10.5" height="8" rx="2.5"/></symbol>
      <symbol id="i-textsize" viewBox="0 0 20 20"><path d="M2.5 15 5.4 8.2 8.3 15M3.6 12.8h3.6M9.5 15l3.9-9.5 3.9 9.5M10.9 11.8h5"/></symbol>
      <symbol id="i-motion" viewBox="0 0 20 20"><circle cx="11" cy="10" r="5.5"/><path d="M3.25 6.5a8 8 0 0 0 0 7M5.25 7.75a5.5 5.5 0 0 0 0 4.5"/></symbol>
      <symbol id="i-contrast" viewBox="0 0 20 20"><circle cx="10" cy="10" r="6.75"/><path d="M10 3.25v13.5a6.75 6.75 0 0 0 0-13.5Z" fill="currentColor"/></symbol>
      <symbol id="i-shapes" viewBox="0 0 20 20"><rect x="3" y="5.5" width="14" height="9" rx="4.5"/><path d="M7.5 10h5"/></symbol>
      <symbol id="i-transparency" viewBox="0 0 20 20"><rect x="3.25" y="3.25" width="9.5" height="9.5" rx="2.2"/><rect x="7.25" y="7.25" width="9.5" height="9.5" rx="2.2" strokeDasharray="1.6 1.9"/></symbol>
      <symbol id="i-voiceover" viewBox="0 0 20 20"><circle cx="10" cy="10" r="7"/><circle cx="10" cy="6.4" r="1" fill="currentColor" stroke="none"/><path d="M6.5 8.3 10 9l3.5-.7M10 9v2.4l-1.8 3.3M10 11.4l1.8 3.3"/></symbol>
      <symbol id="i-location" viewBox="0 0 20 20"><circle cx="10" cy="10" r="7"/><path d="m13.2 6.8-4.6 1.9-1.8 4.5 4.5-1.8Z"/></symbol>
      <symbol id="i-updown" viewBox="0 0 20 20"><path d="m6.75 8 3.25-3.25L13.25 8M6.75 12l3.25 3.25L13.25 12"/></symbol>
      <symbol id="i-phone" viewBox="0 0 20 20"><rect x="5.5" y="2" width="9" height="16" rx="2.2"/><path d="M8.8 4.4h2.4"/></symbol>
      <symbol id="i-tablet" viewBox="0 0 20 20"><rect x="3" y="2.5" width="14" height="15" rx="2"/></symbol>
      <symbol id="i-alert" viewBox="0 0 20 20"><path d="M10 3 17.5 16h-15Z"/><path d="M10 8v3.5M10 13.8v.1"/></symbol>
      <symbol id="i-close" viewBox="0 0 20 20"><path d="m5.5 5.5 9 9M14.5 5.5l-9 9"/></symbol>
      <symbol id="i-screen" viewBox="0 0 20 20"><rect x="2.75" y="4" width="14.5" height="10" rx="2"/><path d="M7 16.5h6"/><path d="m8.6 7.3 3.4 1.7-3.4 1.7Z" fill="currentColor"/></symbol>
      <symbol id="i-eye-off" viewBox="0 0 20 20"><path d="M3 10s2.5-5 7-5 7 5 7 5-2.5 5-7 5-7-5-7-5Z"/><circle cx="10" cy="10" r="2.2"/><path d="m4 3.5 12 13"/></symbol>
      <symbol id="i-folder" viewBox="0 0 20 20"><path d="M2.75 6.25A1.5 1.5 0 0 1 4.25 4.75h3.4l1.6 1.75h6.5a1.5 1.5 0 0 1 1.5 1.5v6.75a1.5 1.5 0 0 1-1.5 1.5H4.25a1.5 1.5 0 0 1-1.5-1.5Z"/></symbol>
      <symbol id="i-chevron" viewBox="0 0 20 20"><path d="m6.5 8.25 3.5 3.5 3.5-3.5"/></symbol>
      <symbol id="i-power" viewBox="0 0 20 20"><path d="M10 3v6.5M6.2 5.6a6 6 0 1 0 7.6 0"/></symbol>
      <symbol id="i-vol-down" viewBox="0 0 20 20"><path d="M3 8v4h3l4 3.5v-11L6 8Z"/><path d="M13 10h4.5"/></symbol>
      <symbol id="i-vol-up" viewBox="0 0 20 20"><path d="M3 8v4h3l4 3.5v-11L6 8Z"/><path d="M13 10h4.5M15.25 7.75v4.5"/></symbol>
      <symbol id="i-check" viewBox="0 0 20 20"><path d="m5 10.5 3.2 3.2L15 6.8"/></symbol>
      <symbol id="i-keyboard" viewBox="0 0 20 20"><rect x="2.25" y="5" width="15.5" height="10" rx="2"/><path d="M5.2 8h.1M8.07 8h.1M10.93 8h.1M13.8 8h.1M5.2 10h.1M8.07 10h.1M10.93 10h.1M13.8 10h.1M6.5 12.4h7"/></symbol>
      <symbol id="i-keyboard-down" viewBox="0 0 20 20"><rect x="2.25" y="3" width="15.5" height="9" rx="2"/><path d="M5.2 6h.1M8.07 6h.1M10.93 6h.1M13.8 6h.1M6.5 9.2h7M7.5 14.5 10 17l2.5-2.5"/></symbol>
      <symbol id="i-clip-send" viewBox="0 0 20 20"><path d="M7.5 3.75h-1.5a1.75 1.75 0 0 0-1.75 1.75v10.25a1.75 1.75 0 0 0 1.75 1.75h8a1.75 1.75 0 0 0 1.75-1.75V5.5a1.75 1.75 0 0 0-1.75-1.75h-1.5"/><rect x="7.5" y="2.5" width="5" height="2.5" rx="1"/><path d="M10 14.25V8.5M7.6 10.9 10 8.5l2.4 2.4"/></symbol>
      <symbol id="i-clip-get" viewBox="0 0 20 20"><path d="M7.5 3.75h-1.5a1.75 1.75 0 0 0-1.75 1.75v10.25a1.75 1.75 0 0 0 1.75 1.75h8a1.75 1.75 0 0 0 1.75-1.75V5.5a1.75 1.75 0 0 0-1.75-1.75h-1.5"/><rect x="7.5" y="2.5" width="5" height="2.5" rx="1"/><path d="M10 8.5v5.75M7.6 11.85 10 14.25l2.4-2.4"/></symbol>
      <symbol id="i-clip-sync" viewBox="0 0 20 20"><path d="M7.5 3.75h-1.5a1.75 1.75 0 0 0-1.75 1.75v10.25a1.75 1.75 0 0 0 1.75 1.75h8a1.75 1.75 0 0 0 1.75-1.75V5.5a1.75 1.75 0 0 0-1.75-1.75h-1.5"/><rect x="7.5" y="2.5" width="5" height="2.5" rx="1"/><path d="M12.7 10.6a2.75 2.75 0 0 0-4.9-1.5M7.3 12.1a2.75 2.75 0 0 0 4.9 1.5M7.6 7.6v1.6h1.6M12.4 15.1v-1.6h-1.6"/></symbol>
      <symbol id="i-download" viewBox="0 0 20 20"><path d="M10 3.5v9M6.5 9 10 12.5 13.5 9M4 15.75h12"/></symbol>
      <symbol id="i-wifi" viewBox="0 0 20 20"><path d="M2.75 8.1a10.5 10.5 0 0 1 14.5 0M5.35 10.9a6.75 6.75 0 0 1 9.3 0M7.95 13.7a3 3 0 0 1 4.1 0"/><circle cx="10" cy="16" r=".9" fill="currentColor" stroke="none"/></symbol>
      <symbol id="i-plug" viewBox="0 0 20 20"><rect x="5.5" y="2" width="9" height="12" rx="2.2"/><path d="M10 14v4M7.5 18h5"/></symbol>
      <symbol id="i-terminal" viewBox="0 0 20 20"><rect x="2.75" y="4" width="14.5" height="12" rx="2.5"/><path d="m6.25 8.25 2 1.75-2 1.75M10.25 12h3.5"/></symbol>
      <symbol id="i-gear" viewBox="0 0 20 20"><circle cx="10" cy="10" r="2.25"/><circle cx="10" cy="10" r="5"/><path d="M10 2.75V5M10 15v2.25M2.75 10H5M15 10h2.25M4.87 4.87l1.6 1.6M13.53 13.53l1.6 1.6M4.87 15.13l1.6-1.6M13.53 6.47l1.6-1.6"/></symbol>
    </svg>
  );
}

/** One icon of the sprite. */
export function Icon({ name, className = "icon" }: { name: string; className?: string }) {
  return (
    <svg className={className} aria-hidden="true">
      <use href={`#${name}`} />
    </svg>
  );
}
