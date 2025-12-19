using System.Runtime.InteropServices;

namespace Stasis.Cli;

[UnmanagedFunctionPointer(CallingConvention.Winapi)]
internal delegate int StasisEntryPoint();
