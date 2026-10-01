using System;
using System.Runtime.InteropServices;

// V4 A107 golden fixture. This is a conformance fixture, not a second lifecycle implementation.
internal static class FfiV1
{
    internal const uint AbiVersion = 1;

    [StructLayout(LayoutKind.Sequential)]
    internal struct TauronBuffer
    {
        internal IntPtr data;
        internal UIntPtr len;
        internal IntPtr owner;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct TauronError
    {
        internal int code;
        internal TauronBuffer message;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct TauronResult
    {
        internal int status;
        internal TauronBuffer buffer;
        internal TauronError error;
    }

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern uint tauron_ffi_abi_version();

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern IntPtr tauron_host_new();

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern IntPtr tauron_host_retain(IntPtr host);

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern void tauron_host_release(IntPtr host);

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int tauron_host_shutdown(IntPtr host);

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern TauronResult tauron_wire_roundtrip_json(
        IntPtr host, IntPtr input, UIntPtr input_len);

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern void tauron_buffer_free(ref TauronBuffer buffer);

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern void tauron_error_free(ref TauronError error);

    [DllImport("tauron_ffi", CallingConvention = CallingConvention.Cdecl)]
    internal static extern void tauron_result_free(ref TauronResult result);
}
