// Drives a console program inside a Windows pseudoconsole (ConPTY, the layer
// Windows Terminal runs programs in) from a small script, keeping a model of
// the screen so steps can wait for text, type keys, resize and dump frames.
//
//   ConPtyDrive.exe <cols> <rows> <script> <workdir> <command line...>
//
// Script lines:
//   type <text>           type text, one key at a time
//   send <text>           send bytes at once; \r \n \t \e \xNN escapes
//   wait <ms> <text>      wait until the screen shows text (FAIL on timeout)
//   gone <ms> <text>      wait until the screen no longer shows text
//   sleep <ms>
//   resize <cols> <rows>
//   dump <label>          print the screen
//   exit <ms>             wait for the program to exit and print its code
// C# 5 (the compiler that ships with .NET Framework).
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using Microsoft.Win32.SafeHandles;

static class Native
{
    [StructLayout(LayoutKind.Sequential)]
    public struct COORD { public short X; public short Y; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct STARTUPINFO
    {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2;
        public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }

    [StructLayout(LayoutKind.Sequential)]
    public struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern int CreatePseudoConsole(COORD size, SafeFileHandle hInput, SafeFileHandle hOutput, uint flags, out IntPtr hPC);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern int ResizePseudoConsole(IntPtr hPC, COORD size);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern void ClosePseudoConsole(IntPtr hPC);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CreatePipe(out SafeFileHandle read, out SafeFileHandle write, IntPtr attributes, int size);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, int flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute, IntPtr value, IntPtr size, IntPtr previous, IntPtr returnSize);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool CreateProcessW(string app, StringBuilder commandLine, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll")]
    public static extern uint WaitForSingleObject(IntPtr handle, uint ms);
    [DllImport("kernel32.dll")]
    public static extern bool SetStdHandle(int which, IntPtr handle);
    [DllImport("kernel32.dll")]
    public static extern bool GetExitCodeProcess(IntPtr handle, out uint code);
}

/// A minimal VT screen: enough of what conhost emits to know what is shown.
class Screen
{
    public int Cols, Rows, R, C;
    char[][] cells;
    int savedR, savedC;
    readonly object gate = new object();

    public Screen(int cols, int rows) { Resize(cols, rows); }

    public void Resize(int cols, int rows)
    {
        lock (gate)
        {
            Cols = cols; Rows = rows;
            cells = new char[rows][];
            for (int i = 0; i < rows; i++) { cells[i] = new char[cols]; Fill(i, 0, cols); }
            R = 0; C = 0;
        }
    }

    void Fill(int row, int from, int to)
    {
        for (int c = Math.Max(0, from); c < Math.Min(Cols, to); c++) cells[row][c] = ' ';
    }

    void ScrollUp(int n)
    {
        for (int k = 0; k < n; k++)
        {
            for (int i = 0; i < Rows - 1; i++) cells[i] = cells[i + 1];
            cells[Rows - 1] = new char[Cols];
            Fill(Rows - 1, 0, Cols);
        }
    }

    void Clamp()
    {
        if (R < 0) R = 0; if (R >= Rows) R = Rows - 1;
        if (C < 0) C = 0; if (C > Cols) C = Cols;
    }

    public string Text()
    {
        lock (gate)
        {
            var sb = new StringBuilder();
            for (int i = 0; i < Rows; i++) sb.Append(new string(cells[i]).TrimEnd()).Append('\n');
            return sb.ToString();
        }
    }

    enum Mode { Ground, Esc, Csi, Osc, OscEsc, Charset }
    Mode mode = Mode.Ground;
    StringBuilder param = new StringBuilder();

    public void Feed(string s)
    {
        lock (gate) { foreach (char ch in s) Step(ch); }
    }

    int P(string[] ps, int i, int dflt)
    {
        int v;
        if (i < ps.Length && int.TryParse(ps[i], out v) && v > 0) return v;
        return dflt;
    }

    void Step(char ch)
    {
        switch (mode)
        {
            case Mode.Ground:
                if (ch == '\x1b') { mode = Mode.Esc; return; }
                if (ch == '\r') { C = 0; return; }
                if (ch == '\n') { R++; if (R >= Rows) { ScrollUp(1); R = Rows - 1; } return; }
                if (ch == '\b') { if (C > 0) C--; return; }
                if (ch == '\t') { C = Math.Min(Cols - 1, (C / 8 + 1) * 8); return; }
                if (ch < ' ' || ch == '\x7f') return;
                if (C >= Cols) { C = 0; R++; if (R >= Rows) { ScrollUp(1); R = Rows - 1; } }
                cells[R][C] = ch; C++;
                return;
            case Mode.Esc:
                if (ch == '[') { mode = Mode.Csi; param.Length = 0; return; }
                if (ch == ']') { mode = Mode.Osc; return; }
                if (ch == '(' || ch == ')') { mode = Mode.Charset; return; }
                if (ch == '7') { savedR = R; savedC = C; }
                else if (ch == '8') { R = savedR; C = savedC; Clamp(); }
                else if (ch == 'M') { if (R > 0) R--; }
                mode = Mode.Ground;
                return;
            case Mode.Charset:
                mode = Mode.Ground;
                return;
            case Mode.Osc:
                if (ch == '\x07') mode = Mode.Ground;
                else if (ch == '\x1b') mode = Mode.OscEsc;
                return;
            case Mode.OscEsc:
                mode = ch == '\\' ? Mode.Ground : Mode.Osc;
                return;
            case Mode.Csi:
                if (ch >= 0x40 && ch <= 0x7e) { Csi(ch, param.ToString()); mode = Mode.Ground; }
                else param.Append(ch);
                return;
        }
    }

    void Csi(char final, string raw)
    {
        if (raw.StartsWith("?") || raw.StartsWith(">") || raw.StartsWith("=")) return;
        string[] ps = raw.Split(';');
        int n = P(ps, 0, 1);
        switch (final)
        {
            case 'H': case 'f': R = P(ps, 0, 1) - 1; C = P(ps, 1, 1) - 1; break;
            case 'A': R -= n; break;
            case 'B': R += n; break;
            case 'C': C += n; break;
            case 'D': C -= n; break;
            case 'E': R += n; C = 0; break;
            case 'F': R -= n; C = 0; break;
            case 'G': C = n - 1; break;
            case 'd': R = n - 1; break;
            case 'J':
            {
                int m = ps[0] == "" ? 0 : P(ps, 0, 0);
                if (m == 2 || m == 3) { for (int i = 0; i < Rows; i++) Fill(i, 0, Cols); }
                else if (m == 0) { Fill(R, C, Cols); for (int i = R + 1; i < Rows; i++) Fill(i, 0, Cols); }
                else if (m == 1) { for (int i = 0; i < R; i++) Fill(i, 0, Cols); Fill(R, 0, C + 1); }
                break;
            }
            case 'K':
            {
                int m = ps[0] == "" ? 0 : P(ps, 0, 0);
                if (m == 0) Fill(R, C, Cols); else if (m == 1) Fill(R, 0, C + 1); else Fill(R, 0, Cols);
                break;
            }
            case 'X': Fill(R, C, C + n); break;
            case 'L':
                for (int k = 0; k < n; k++)
                {
                    for (int i = Rows - 1; i > R; i--) cells[i] = cells[i - 1];
                    cells[R] = new char[Cols]; Fill(R, 0, Cols);
                }
                break;
            case 'M':
                for (int k = 0; k < n; k++)
                {
                    for (int i = R; i < Rows - 1; i++) cells[i] = cells[i + 1];
                    cells[Rows - 1] = new char[Cols]; Fill(Rows - 1, 0, Cols);
                }
                break;
            case 'S': ScrollUp(n); break;
            case 'P':
            {
                var row = cells[R];
                for (int c = C; c < Cols; c++) row[c] = c + n < Cols ? row[c + n] : ' ';
                break;
            }
            case '@':
            {
                var row = cells[R];
                for (int c = Cols - 1; c >= C + n; c--) row[c] = row[c - n];
                Fill(R, C, C + n);
                break;
            }
            default: break; // SGR, modes, scroll regions: no effect on the text
        }
        Clamp();
    }
}

static class Program
{
    static Screen screen;
    static FileStream input;
    static IntPtr pc;
    static Native.PROCESS_INFORMATION proc;
    static volatile bool exited;
    // CONPTY_RAW=<file>: also append everything the console sends to it.
    static string rawPath = Environment.GetEnvironmentVariable("CONPTY_RAW");

    static string Unescape(string s)
    {
        var sb = new StringBuilder();
        for (int i = 0; i < s.Length; i++)
        {
            if (s[i] != '\\' || i + 1 >= s.Length) { sb.Append(s[i]); continue; }
            char e = s[++i];
            if (e == 'r') sb.Append('\r');
            else if (e == 'n') sb.Append('\n');
            else if (e == 't') sb.Append('\t');
            else if (e == 'e') sb.Append('\x1b');
            else if (e == 'x' && i + 2 < s.Length) { sb.Append((char)Convert.ToInt32(s.Substring(i + 1, 2), 16)); i += 2; }
            else sb.Append(e);
        }
        return sb.ToString();
    }

    static void Send(string text)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(text);
        input.Write(bytes, 0, bytes.Length);
        input.Flush();
    }

    static void Dump(string label)
    {
        Console.WriteLine("----- screen: " + label + " (" + screen.Cols + "x" + screen.Rows + ") -----");
        Console.Write(screen.Text());
        Console.WriteLine("----- end -----");
    }

    static bool WaitFor(int ms, Func<bool> condition)
    {
        var watch = Stopwatch.StartNew();
        while (watch.ElapsedMilliseconds < ms)
        {
            if (condition()) return true;
            Thread.Sleep(50);
        }
        return condition();
    }

    static void KillTree()
    {
        try
        {
            var kill = Process.Start(new ProcessStartInfo("taskkill", "/T /F /PID " + proc.dwProcessId) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true });
            kill.WaitForExit(10000);
        }
        catch { }
    }

    static int Main(string[] args)
    {
        if (args.Length < 5) { Console.Error.WriteLine("usage: ConPtyDrive <cols> <rows> <script> <workdir> <command line...>"); return 2; }
        Console.OutputEncoding = new UTF8Encoding(false);
        int cols = int.Parse(args[0]), rows = int.Parse(args[1]);
        string[] script = File.ReadAllLines(args[2]);
        string workdir = args[3];
        string commandLine = string.Join(" ", args, 4, args.Length - 4);
        screen = new Screen(cols, rows);

        SafeFileHandle inRead, inWrite, outRead, outWrite;
        Native.CreatePipe(out inRead, out inWrite, IntPtr.Zero, 0);
        Native.CreatePipe(out outRead, out outWrite, IntPtr.Zero, 0);
        var size = new Native.COORD { X = (short)cols, Y = (short)rows };
        int hr = Native.CreatePseudoConsole(size, inRead, outWrite, 0, out pc);
        if (hr != 0) { Console.Error.WriteLine("CreatePseudoConsole failed: " + hr); return 2; }

        var listSize = IntPtr.Zero;
        Native.InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref listSize);
        var si = new Native.STARTUPINFOEX();
        si.StartupInfo.cb = Marshal.SizeOf(typeof(Native.STARTUPINFOEX));
        si.lpAttributeList = Marshal.AllocHGlobal(listSize);
        Native.InitializeProcThreadAttributeList(si.lpAttributeList, 1, 0, ref listSize);
        Native.UpdateProcThreadAttribute(si.lpAttributeList, 0, (IntPtr)0x00020016, pc, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);
        // A child inherits this process's standard handles when they are not a
        // console (this driver's output is usually redirected), and would then
        // write there instead of to the pseudoconsole. Clear them for the
        // launch; Console.Out keeps its own copy.
        Console.Out.Flush();
        Console.Error.Flush();
        Native.SetStdHandle(-10, IntPtr.Zero);
        Native.SetStdHandle(-11, IntPtr.Zero);
        Native.SetStdHandle(-12, IntPtr.Zero);
        if (!Native.CreateProcessW(null, new StringBuilder(commandLine), IntPtr.Zero, IntPtr.Zero, false, 0x00080000, IntPtr.Zero, workdir, ref si, out proc))
        {
            Console.Error.WriteLine("CreateProcess failed: " + Marshal.GetLastWin32Error());
            return 2;
        }
        inRead.Dispose();
        outWrite.Dispose();
        input = new FileStream(inWrite, FileAccess.Write);

        var reader = new Thread(() =>
        {
            var stream = new FileStream(outRead, FileAccess.Read);
            var decoder = Encoding.UTF8.GetDecoder();
            var buffer = new byte[8192];
            var chars = new char[8192 * 2];
            try
            {
                int read;
                while ((read = stream.Read(buffer, 0, buffer.Length)) > 0)
                {
                    int count = decoder.GetChars(buffer, 0, read, chars, 0);
                    string text = new string(chars, 0, count);
                    if (rawPath != null) File.AppendAllText(rawPath, text, new UTF8Encoding(false));
                    // Keep reading whatever the screen model makes of it: a reader that
                    // stops leaves the pseudoconsole's output pipe unread, and conhost
                    // then drops every process in it (writes fail with error 233).
                    try { screen.Feed(text); }
                    catch (Exception error) { Console.Error.WriteLine("screen model: " + error.Message); }
                }
            }
            catch { }
        });
        reader.IsBackground = true;
        reader.Start();
        var watcher = new Thread(() => { Native.WaitForSingleObject(proc.hProcess, 0xFFFFFFFF); exited = true; });
        watcher.IsBackground = true;
        watcher.Start();

        int failures = 0;
        foreach (string rawLine in script)
        {
            string line = rawLine.TrimEnd();
            if (line.Length == 0 || line.StartsWith("#")) continue;
            int space = line.IndexOf(' ');
            string verb = space < 0 ? line : line.Substring(0, space);
            string rest = space < 0 ? "" : line.Substring(space + 1);
            if (verb == "type")
            {
                foreach (char ch in Unescape(rest)) { Send(ch.ToString()); Thread.Sleep(15); }
                Console.WriteLine("ok   type " + rest);
            }
            else if (verb == "send") { Send(Unescape(rest)); Console.WriteLine("ok   send " + rest); }
            else if (verb == "sleep") { Thread.Sleep(int.Parse(rest)); }
            else if (verb == "resize")
            {
                string[] parts = rest.Split(' ');
                short c = short.Parse(parts[0]), r = short.Parse(parts[1]);
                screen.Resize(c, r);
                Native.ResizePseudoConsole(pc, new Native.COORD { X = c, Y = r });
                Console.WriteLine("ok   resize " + rest);
            }
            else if (verb == "dump") { Dump(rest); }
            else if (verb == "wait" || verb == "gone")
            {
                int at = rest.IndexOf(' ');
                int ms = int.Parse(rest.Substring(0, at));
                string text = rest.Substring(at + 1);
                bool want = verb == "wait";
                bool ok = WaitFor(ms, () => screen.Text().Contains(text) == want);
                Console.WriteLine((ok ? "ok   " : "FAIL ") + verb + " " + text);
                if (!ok) { failures++; Dump("after failed " + verb); break; }
            }
            else if (verb == "exit")
            {
                bool ok = WaitFor(int.Parse(rest), () => exited);
                uint code = 0;
                if (ok) Native.GetExitCodeProcess(proc.hProcess, out code);
                Console.WriteLine((ok ? "ok   exit code " + code : "FAIL exit (still running)"));
                if (!ok) { failures++; Dump("still running"); }
            }
            else { Console.WriteLine("FAIL unknown step: " + line); failures++; break; }
        }

        Thread.Sleep(300);
        Dump("final");
        if (!exited) KillTree();
        Native.ClosePseudoConsole(pc);
        Console.WriteLine(failures == 0 ? "RESULT pass" : "RESULT fail (" + failures + ")");
        return failures == 0 ? 0 : 1;
    }
}
