# YAAC Windows WPF renderer

Choose **Windows WPF** under **Settings → Window → Renderer** and restart YAAC.
To choose WPF for a single start, run `yaac.exe --renderer windows-wpf`. The Windows
installer includes its self-contained .NET runtime. The WPF window uses the same Rust
controller and hub connection as the standard window. The renderer and controller exchange
messages over anonymous pipes.

Windows tray behavior follows **Keep running in the tray when closed**. The tray icon opens
YAAC, shows when a thread needs attention, and offers **Quit** in its menu.

To test with demo data on Windows:

```powershell
.\yaac.exe --demo fleet --renderer windows-wpf --theme dark
.\yaac.exe --demo fresh --renderer windows-wpf --theme dark
```

**Demo: play the story** includes the standard fleet scenarios and **Mark finished agents unread**.
Use the latter to try **Mark all as read** in the related-agents card or dock.
**Stream a reply in this thread** runs the existing scripted demo turn. Choose an idle thread for the test.

In a demo thread, use the composer to exercise Markdown styling, drop an image to see the
attachment thumbnail, and open a finished turn to reveal its tool activity and file changes.

## Build

From the repository root in Windows PowerShell with Rust and the .NET 10 SDK:

```powershell
cargo build --locked --release -p yaac-app --bin yaac
dotnet publish tools/wpf-preview/Yaac.WpfPreview.csproj -c Release -r win-x64 --self-contained -o dist/wpf-preview/wpf
Copy-Item target/release/yaac.exe dist/wpf-preview/yaac.exe
```

Keep the entire output folder together. To test a native local provider, use an installer build
or include the Windows YAAC daemon and its Node runtime as described in the install guide.
