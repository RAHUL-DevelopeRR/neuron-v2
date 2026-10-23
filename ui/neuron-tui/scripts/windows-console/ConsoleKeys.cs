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
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetNumberOfConsoleInputEvents(IntPtr handle, out uint count);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern uint GetConsoleTitleW(StringBuilder title, uint size);

    const uint ReadWrite = 0xC0000000, ShareReadWrite = 3, OpenExisting = 3;

    // The process tree from a toolhelp snapshot: fast, and independent of the
    // WMI service (Get-CimInstance Win32_Process), which can stall for minutes
    // on a loaded machine.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct ProcessEntry
    {
        public uint Size, Usage, ProcessId;
        public IntPtr DefaultHeapId;
        public uint ModuleId, Threads, ParentProcessId;
        public int PriorityClassBase;
        public uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string ExeFile;
    }
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool Process32FirstW(IntPtr snapshot, ref ProcessEntry entry);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool Process32NextW(IntPtr snapshot, ref ProcessEntry entry);

    static System.Collections.Generic.List<ProcessEntry> Snapshot()
    {
        var all = new System.Collections.Generic.List<ProcessEntry>();
        IntPtr snapshot = CreateToolhelp32Snapshot(2, 0);
        if (snapshot == new IntPtr(-1)) return all;
        try
        {
            var entry = new ProcessEntry { Size = (uint)Marshal.SizeOf(typeof(ProcessEntry)) };
            for (bool more = Process32FirstW(snapshot, ref entry); more; more = Process32NextW(snapshot, ref entry)) all.Add(entry);
        }
        finally { CloseHandle(snapshot); }
        return all;
    }

    /// `root` and every process below it.
    public static uint[] Descendants(uint root)
    {
        var all = Snapshot();
        var found = new System.Collections.Generic.List<uint> { root };
        for (int i = 0; i < found.Count; i++)
            foreach (var entry in all)
                if (entry.ParentProcessId == found[i] && entry.ProcessId != found[i] && !found.Contains(entry.ProcessId)) found.Add(entry.ProcessId);
        return found.ToArray();
    }

    /// The first child of `parent` named `exe` (e.g. "cmd.exe"); 0 when there is none.
    public static uint ChildNamed(uint parent, string exe)
    {
        foreach (var entry in Snapshot())
            if (entry.ParentProcessId == parent && string.Equals(entry.ExeFile, exe, StringComparison.OrdinalIgnoreCase)) return entry.ProcessId;
        return 0;
    }

    /// The UI's engine below `root`: a neuron.exe that neuron-tui.exe started; 0 when there is none.
    public static uint EngineBelow(uint root)
    {
        var all = Snapshot();
        var below = new System.Collections.Generic.HashSet<uint>(Descendants(root));
        foreach (var entry in all)
        {
            if (!below.Contains(entry.ProcessId) || !string.Equals(entry.ExeFile, "neuron.exe", StringComparison.OrdinalIgnoreCase)) continue;
            foreach (var parent in all)
                if (parent.ProcessId == entry.ParentProcessId && string.Equals(parent.ExeFile, "neuron-tui.exe", StringComparison.OrdinalIgnoreCase)) return entry.ProcessId;
        }
        return 0;
    }

    /// The names of `root` and the processes below it, for a failure report.
    public static string Names(uint root)
    {
        var below = new System.Collections.Generic.HashSet<uint>(Descendants(root));
        var names = new System.Collections.Generic.List<string>();
        foreach (var entry in Snapshot())
            if (below.Contains(entry.ProcessId)) names.Add(entry.ExeFile + "#" + entry.ProcessId);
        return string.Join(", ", names);
    }

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

    /// The console window's title; null when the console is gone.
    public static string Title(uint pid)
    {
        if (!Attach(pid)) { FreeConsole(); return null; }
        try
        {
            var title = new StringBuilder(1024);
            GetConsoleTitleW(title, (uint)title.Capacity);
            return title.ToString();
        }
        finally { FreeConsole(); }
    }

    /// Input events written to the console that no process has read yet; -1 when
    /// the console is gone.
    public static int PendingInput(uint pid)
    {
        if (!Attach(pid)) { FreeConsole(); return -1; }
        IntPtr input = CreateFileW("CONIN$", ReadWrite, ShareReadWrite, IntPtr.Zero, OpenExisting, 0, IntPtr.Zero);
        try
        {
            uint count;
            if (input == new IntPtr(-1) || !GetNumberOfConsoleInputEvents(input, out count)) return -1;
            return (int)count;
        }
        finally
        {
            if (input != new IntPtr(-1)) CloseHandle(input);
            FreeConsole();
        }
    }

    /// Types `text` as key presses (a key down and its key up each): `\r` is Enter,
    /// `\b` is Backspace, `\x03` is Ctrl-C. False when the console is gone.
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
                else if (ch == '\b') key = 0x08;
                else if (ch == ' ') key = 0x20;
                else if (ch >= '0' && ch <= '9') key = ch;
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
