---
name: devicesteer
description: Operate a real iPhone or iPad connected over USB or Wi-Fi, and set a new one up (trust, Developer Mode, Developer Disk Image) without Xcode. Read the screen's accessibility tree, tap, swipe, type text (Latin and Hangul) and keys, read or set the clipboard, press buttons, launch apps, change appearance, accessibility settings and the simulated location, and record the screen, in a session the person watching in the DeviceSteer viewer can take over or stop. Use it when a task needs a physical device, not a simulator.
metadata:
  bootstrap: "true"
---

# DeviceSteer

Before any device task, run:

```bash
devicesteer skill
```

and follow its output. It prints the usage guide that matches the installed `devicesteer` version.
Run it again after updating `devicesteer`, or when any output says to.

The guide ends with a line starting `[devicesteer:skill] end of guide`. If that line is missing, the
output was truncated: save it to a file and read the file in parts, for example
`devicesteer skill > "$TMPDIR/devicesteer-skill.md"`.

Points (`ui` frames, `tap x,y`, `swipe`) are always screen points in the orientation the screen
shows, portrait or landscape; the guide explains it.

A device that isn't ready yet (`devices` lists it with `setup`) is set up with `devicesteer setup`;
the guide says which steps need the person holding the device.

If `devicesteer` is not found, build it from this repository (`cargo build`, then use
`target/debug/devicesteer`) or ask the user how it is installed.
