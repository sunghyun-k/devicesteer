// The hero: a recording of an agent session on a real device, drawn inside the viewer. The
// overlay (the agent's arrow, its press and its labels) follows the session's timeline, synced to
// the video's own clock so it stays aligned through every loop. Without this script, or with
// reduced motion, the poster shows with the arrow and the first label standing still.
(() => {
  // Where the hero's media lives: the video files and the poster. Point it at external storage
  // when the files move there (and the poster's src in index.html with it).
  const MEDIA_BASE = '/media/';
  // The session, from the recording's timeline (video offsets in ms, points as fractions of the
  // screen). A tap's arrow glides in for GLIDE_MS before the finger goes down at t.
  const GOAL = 'Turn on dark mode';
  const INPUTS = [
    { t: 4719, kind: 'tap', at: [0.3148, 0.1406], label: 'Open the Settings app' },
    { t: 9393, kind: 'tap', at: [0.4313, 0.5137], label: 'Open Appearance settings' },
    { t: 13072, kind: 'tap', at: [0.5815, 0.5147], label: 'Switch to Dark mode' },
    { t: 15661, kind: 'button', label: 'Go back to Home' },
  ];
  // The viewer's timings (app/src/main.ts and style.css): the finger is down 50 ms, its disc fades
  // over 350 ms, and a label shows for 2.6 s. The glide is longer than the viewer's 160 ms so the
  // move reads on a page.
  const GLIDE_MS = 320;
  const HOLD_MS = 50;
  const RELEASE_MS = 350;
  const LABEL_MS = 2600;

  const root = document.getElementById('hero-viewer');
  if (!root) return;
  const screen = root.querySelector('.hv-screen');
  const overlay = root.querySelector('.hv-overlay');
  const cursor = overlay.querySelector('.vcursor');
  const press = cursor.querySelector('.press');
  const arrow = cursor.querySelector('.arrow');
  const tipLabel = cursor.querySelector('.vlabel');
  const buttonLabel = overlay.querySelector('.vlabel.bottom');
  const poster = screen.querySelector('.hv-poster');
  root.querySelector('.vgoal').textContent = GOAL;
  // The markup puts the arrow at the first tap for the still; from here it moves by transform.
  cursor.style.left = cursor.style.top = '0';
  poster.src = MEDIA_BASE + 'hero-poster.webp';

  const taps = INPUTS.filter((i) => i.kind === 'tap');
  const ease = (p) => 1 - (1 - p) ** 3;
  let labelText = null;

  // Where the overlay is at video time `ms`: everything is worked out from the time alone, so a
  // seek or a loop needs no bookkeeping.
  function render(ms) {
    const w = overlay.clientWidth;
    const h = overlay.clientHeight;
    const next = taps.find((i) => i.t > ms);
    const prev = [...taps].reverse().find((i) => i.t <= ms);
    let at = prev?.at;
    let opacity = prev ? 1 : 0;
    if (next && ms >= next.t - GLIDE_MS) {
      const p = ease((ms - (next.t - GLIDE_MS)) / GLIDE_MS);
      if (prev) at = [prev.at[0] + (next.at[0] - prev.at[0]) * p, prev.at[1] + (next.at[1] - prev.at[1]) * p];
      else { at = next.at; opacity = Math.min(1, (ms - (next.t - GLIDE_MS)) / 150); }
    }
    cursor.hidden = !at;
    if (at) {
      cursor.style.opacity = opacity;
      cursor.style.transform = `translate(${at[0] * w}px, ${at[1] * h}px)`;
    }

    // The press: the disc shows while the finger is down, then grows and fades.
    const down = prev && ms - prev.t;
    if (down !== undefined && down < HOLD_MS + RELEASE_MS) {
      const r = Math.max(0, (down - HOLD_MS) / RELEASE_MS);
      press.style.opacity = 1 - ease(r);
      press.style.transform = `scale(${1 + 0.4 * ease(r)})`;
      arrow.style.transform = down < HOLD_MS ? 'scale(0.88)' : '';
    } else {
      press.style.opacity = 0;
      arrow.style.transform = '';
    }

    // The label of the latest input, by the arrow for a tap and at the bottom for a button.
    const shown = [...INPUTS].reverse().find((i) => {
      const from = i.kind === 'tap' ? i.t - GLIDE_MS : i.t;
      return ms >= from && ms < from + LABEL_MS;
    });
    const tap = shown?.kind === 'tap';
    tipLabel.hidden = !tap;
    buttonLabel.hidden = !shown || tap;
    if (shown) {
      const label = tap ? tipLabel : buttonLabel;
      if (labelText !== shown.label) {
        labelText = shown.label;
        label.textContent = shown.label;
      }
      const p = (ms - (tap ? shown.t - GLIDE_MS : shown.t)) / LABEL_MS;
      label.style.opacity = p < 0.08 ? p / 0.08 : p < 0.75 ? 1 : 1 - (p - 0.75) / 0.25;
      if (tap) {
        // As in the viewer: below and right of the tip. Where that would leave the window (a
        // narrow page), it slides left, staying under the arrow.
        const stage = root.querySelector('.hv-stage').getBoundingClientRect();
        const x = overlay.getBoundingClientRect().left + at[0] * w;
        const room = stage.right - 8 - (x + 16 + label.offsetWidth);
        const left = Math.max(stage.left + 8 - x, 16 + Math.min(0, room));
        label.style.left = left + 'px';
      }
    }
  }

  // The still: the first tap with its label, as the reduced-motion and no-video picture.
  function still() {
    const first = taps[0];
    render(first.t + HOLD_MS + RELEASE_MS);
    tipLabel.style.opacity = 1;
  }

  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  if (reduced.matches) {
    still();
    addEventListener('resize', still);
    return;
  }

  const video = document.createElement('video');
  Object.assign(video, { muted: true, loop: true, autoplay: true, playsInline: true, preload: 'auto' });
  video.setAttribute('muted', '');
  video.setAttribute('playsinline', '');
  video.poster = MEDIA_BASE + 'hero-poster.webp';
  video.setAttribute('aria-hidden', 'true');
  video.className = 'hv-video';
  for (const [file, type] of [['hero.webm', 'video/webm; codecs="av01.0.05M.08"'], ['hero.mp4', 'video/mp4; codecs="avc1.640020"']]) {
    const s = document.createElement('source');
    s.src = MEDIA_BASE + file;
    s.type = type;
    video.append(s);
  }
  screen.append(video);
  render(0);
  video.addEventListener('playing', () => root.classList.add('playing'), { once: true });

  let frame = 0;
  const tick = () => {
    render(video.currentTime * 1000);
    frame = requestAnimationFrame(tick);
  };
  video.addEventListener('play', () => { cancelAnimationFrame(frame); tick(); });
  video.addEventListener('pause', () => cancelAnimationFrame(frame));
  video.addEventListener('seeked', () => render(video.currentTime * 1000));
  // Off screen, the video stops and so does the overlay's loop.
  new IntersectionObserver(([e]) => {
    if (e.isIntersecting) video.play().catch(() => {});
    else video.pause();
  }).observe(root);
  // The video failed (no media at MEDIA_BASE, or no format it plays): keep the still.
  video.addEventListener('error', () => still(), true);
  reduced.addEventListener('change', () => { if (reduced.matches) { video.pause(); still(); } });
})();
