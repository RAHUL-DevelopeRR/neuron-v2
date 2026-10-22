// Reads a classic console window's screen and types into it, from outside: it
// attaches to the console of a process running there (AttachConsole), reads the
// buffer with ReadConsoleOutputCharacter and writes key events with
// WriteConsoleInput. classic-console-check.ps1 loads it with Add-Type.
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class ConsoleKeys
{
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr handle);

    [StructLayout(LayoutKind.Sequential)] struct Coord { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential)] struct SmallRect { public short Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    struct BufferInfo { public Coord Size; public Coord Cursor; public ushort Attributes; public SmallRect Window; public Coord MaxWindow; }
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetConsoleScreenBufferInfo(IntPtr handle, out BufferInfo info);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool ReadConsoleOutputCharacterW(IntPtr handle, StringBuilder text, uint length, Coord at, out uint read);

    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    struct KeyRecord
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public int KeyDown;
        [FieldOffset(8)] public ushort RepeatCount;
        [FieldOffset(10)] public ushort VirtualKey;
        [FieldOffset(12)] public ushort ScanCode;
        [FieldOffset(14)] public char Char;
        [FieldOffset(16)] public uint ControlKeys;
    }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool WriteConsoleInputW(IntPtr handle, KeyRecord[] records, uint count, out uint written);

    const uint ReadWrite = 0xC0000000, ShareReadWrite = 3, OpenExisting = 3;

    static bool Attach(uint pid)
    {
        FreeConsole();
        if (!AttachConsole(pid)) return false;
        // Ctrl-C typed into that console must not stop this process.
        SetConsoleCtrlHandler(IntPtr.Zero, true);
        return true;
    }

    /// The visible window of the console `pid` runs in; null when it is gone.
    public static string Screen(uint pid)
    {
        if (!Attach(pid)) { FreeConsole(); return null; }
        IntPtr output = CreateFileW("CONOUT$", ReadWrite, ShareReadWrite, IntPtr.Zero, OpenExisting, 0, IntPtr.Zero);
        try
        {
            BufferInfo info;
            if (output == new IntPtr(-1) || !GetConsoleScreenBufferInfo(output, out info)) return null;
            var screen = new StringBuilder();
            for (short y = info.Window.Top; y <= info.Window.Bottom; y++)
            {
                var line = new StringBuilder(info.Size.X + 1);
                uint read;
                ReadConsoleOutputCharacterW(output, line, (uint)info.Size.X, new Coord { X = 0, Y = y }, out read);
                string text = line.ToString();
                if (text.Length > (int)read) text = text.Substring(0, (int)read);
                screen.Append(text.TrimEnd()).Append('\n');
            }
            return screen.ToString();
        }
        finally
        {
            if (output != new IntPtr(-1)) CloseHandle(output);
            FreeConsole();
        }
    }

    /// Types `text` as key presses (a key down and its key up each): `\r` is Enter,
    /// `\x03` is Ctrl-C. False when the console is gone.
    public static bool Type(uint pid, string text)
    {
        if (!Attach(pid)) { FreeConsole(); return false; }
        IntPtr input = CreateFileW("CONIN$", ReadWrite, ShareReadWrite, IntPtr.Zero, OpenExisting, 0, IntPtr.Zero);
        try
        {
            if (input == new IntPtr(-1)) return false;
            foreach (char ch in text)
            {
                ushort key = 0;
                uint controls = 0;
                if (ch == '\r') key = 0x0D;
                else if (ch == (char)3) { key = 0x43; controls = 0x0008; }
                else if (ch >= 'a' && ch <= 'z') key = (ushort)(ch - 32);
                else if (ch >= 'A' && ch <= 'Z') { key = ch; controls = 0x0010; }
                else if (ch == '/') key = 0xBF;
                var down = new KeyRecord { EventType = 1, KeyDown = 1, RepeatCount = 1, VirtualKey = key, Char = ch, ControlKeys = controls };
                var up = down;
                up.KeyDown = 0;
                uint written;
                if (!WriteConsoleInputW(input, new[] { down, up }, 2, out written)) return false;
                System.Threading.Thread.Sleep(30);
            }
            return true;
        }
        finally
        {
            if (input != new IntPtr(-1)) CloseHandle(input);
            FreeConsole();
        }
    }
}
