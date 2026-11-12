// YftWriter: the write half of the vehicle optimizer. cw-core (Rust) is read-only,
// so index buffers computed there (meshoptimizer) are applied here, with
// CodeWalker.Core, which can serialise a .yft.
//
//   YftWriter info  <in.yft>
//   YftWriter apply <in.yft> <plan.bin> <out.yft>
//
// plan.bin (little-endian):
//   u32 magic 'YPLN' (0x4E4C5059), u32 version (2), u32 entries
//   per entry: u32 drawable, u32 lod (0 high, 1 med, 2 low, 3 vlow),
//              u32 model, u32 geometry, u32 count, count x u32 index
// Indices always refer to the vertices of the ORIGINAL high-LOD geometry.
// Drawable order matches cw-core `read_named_fragments`: drawable, cloth,
// drawable array, then the children of each physics LOD (drawable1, drawable2).
//
// Vertex buffers are compacted (only referenced vertices are kept). Medium,
// low and very-low LOD models that do not exist are created from the high
// LOD's models. Stdout: one JSON line with before/after counts.
//
// C# 5 on purpose: built with the csc that ships with .NET Framework (see build.cmd).

using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
using System.Text;
using System.Threading;
using CodeWalker.GameFiles;

public static class YftWriter
{
    static int Main(string[] args)
    {
        // CodeWalker.Core and SharpDX live next to CodeWalker; find them there.
        string dir = Environment.GetEnvironmentVariable("CODEWALKER_DIR");
        if (string.IsNullOrEmpty(dir))
            dir = @"E:\Proyectos\Astra\CodeWalkerAstra\CodeWalker\bin\Release\net48";
        AppDomain.CurrentDomain.AssemblyResolve += delegate (object s, ResolveEventArgs a)
        {
            string p = Path.Combine(dir, a.Name.Split(',')[0] + ".dll");
            return File.Exists(p) ? Assembly.LoadFrom(p) : null;
        };

        int code = 0;
        // CodeWalker's resource reader is deeply recursive: give it a big stack.
        var t = new Thread(delegate ()
        {
            try { code = Run(args); }
            catch (Exception e) { Console.Error.WriteLine(e.ToString()); code = 1; }
        }, 512 * 1024 * 1024);
        t.Start();
        t.Join();
        return code;
    }

    static int Run(string[] args)
    {
        if (args.Length >= 2 && args[0] == "info")
        {
            var yft = Load(args[1]);
            Console.WriteLine(Describe(yft, "info"));
            return 0;
        }
        if (args.Length >= 4 && args[0] == "apply")
        {
            var yft = Load(args[1]);
            string before = Describe(yft, "before");
            Apply(yft, args[2]);
            byte[] bytes = yft.Save();
            File.WriteAllBytes(args[3], bytes);
            string after = Describe(yft, "after");
            Console.WriteLine("{" + before + "," + after + ",\"bytes_out\":" + bytes.Length + "}");
            return 0;
        }
        Console.Error.WriteLine("usage: YftWriter info <in.yft> | apply <in.yft> <plan.bin> <out.yft>");
        return 2;
    }

    static YftFile Load(string path)
    {
        var yft = new YftFile();
        yft.Load(File.ReadAllBytes(path));
        if (yft.Fragment == null) throw new Exception("not a loadable fragment: " + path);
        return yft;
    }

    static List<DrawableBase> Drawables(FragType f)
    {
        var l = new List<DrawableBase>();
        if (f.Drawable != null) l.Add(f.Drawable);
        if (f.DrawableCloth != null) l.Add(f.DrawableCloth);
        if (f.DrawableArray != null && f.DrawableArray.data_items != null)
            foreach (var d in f.DrawableArray.data_items) if (d != null) l.Add(d);
        var g = f.PhysicsLODGroup;
        if (g != null)
        {
            var lods = new FragPhysicsLOD[] { g.PhysicsLOD1, g.PhysicsLOD2, g.PhysicsLOD3 };
            foreach (var lod in lods)
            {
                if (lod == null || lod.Children == null || lod.Children.data_items == null) continue;
                foreach (var c in lod.Children.data_items)
                {
                    if (c == null) continue;
                    if (c.Drawable1 != null) l.Add(c.Drawable1);
                    if (c.Drawable2 != null) l.Add(c.Drawable2);
                }
            }
        }
        return l;
    }

    static DrawableModel[] GetLod(DrawableBase d, int lod)
    {
        var m = d.DrawableModels;
        if (m == null) return null;
        switch (lod)
        {
            case 0: return m.High;
            case 1: return m.Med;
            case 2: return m.Low;
            default: return m.VLow;
        }
    }

    static void SetLod(DrawableBase d, int lod, DrawableModel[] models)
    {
        switch (lod)
        {
            case 0: d.DrawableModels.High = models; break;
            case 1: d.DrawableModels.Med = models; break;
            case 2: d.DrawableModels.Low = models; break;
            default: d.DrawableModels.VLow = models; break;
        }
    }

    static long Tris(DrawableModel[] models)
    {
        long n = 0;
        if (models == null) return 0;
        foreach (var m in models)
        {
            if (m == null || m.Geometries == null) continue;
            foreach (var g in m.Geometries)
                if (g != null && g.IndexBuffer != null) n += g.IndexBuffer.Indices != null ? g.IndexBuffer.Indices.Length / 3 : 0;
        }
        return n;
    }

    static long Verts(DrawableModel[] models)
    {
        long n = 0;
        if (models == null) return 0;
        foreach (var m in models)
        {
            if (m == null || m.Geometries == null) continue;
            foreach (var g in m.Geometries)
                if (g != null && g.VertexData != null) n += g.VertexData.VertexCount;
        }
        return n;
    }

    static string Describe(YftFile yft, string key)
    {
        var sb = new StringBuilder();
        sb.Append("\"" + key + "\":[");
        var ds = Drawables(yft.Fragment);
        for (int i = 0; i < ds.Count; i++)
        {
            var d = ds[i];
            if (i > 0) sb.Append(",");
            sb.Append("{\"drawable\":" + i);
            sb.Append(",\"tris\":[" + Tris(GetLod(d, 0)) + "," + Tris(GetLod(d, 1)) + "," + Tris(GetLod(d, 2)) + "," + Tris(GetLod(d, 3)) + "]");
            sb.Append(",\"verts\":[" + Verts(GetLod(d, 0)) + "," + Verts(GetLod(d, 1)) + "," + Verts(GetLod(d, 2)) + "," + Verts(GetLod(d, 3)) + "]");
            sb.Append(",\"lod_dist\":[" + F(d.LodDistHigh) + "," + F(d.LodDistMed) + "," + F(d.LodDistLow) + "," + F(d.LodDistVlow) + "]}");
        }
        sb.Append("]");
        return sb.ToString();
    }

    static string F(float v)
    {
        return v.ToString("0.###", System.Globalization.CultureInfo.InvariantCulture);
    }

    struct Entry
    {
        public int Drawable, Lod, Model, Geometry;
        public uint[] Indices;
    }

    static List<Entry> ReadPlan(string path)
    {
        var list = new List<Entry>();
        using (var br = new BinaryReader(File.OpenRead(path)))
        {
            if (br.ReadUInt32() != 0x4E4C5059) throw new Exception("bad plan magic");
            if (br.ReadUInt32() != 2) throw new Exception("unsupported plan version");
            uint n = br.ReadUInt32();
            for (uint i = 0; i < n; i++)
            {
                var e = new Entry();
                e.Drawable = (int)br.ReadUInt32();
                e.Lod = (int)br.ReadUInt32();
                e.Model = (int)br.ReadUInt32();
                e.Geometry = (int)br.ReadUInt32();
                uint cnt = br.ReadUInt32();
                e.Indices = new uint[cnt];
                for (uint k = 0; k < cnt; k++) e.Indices[k] = br.ReadUInt32();
                list.Add(e);
            }
        }
        return list;
    }

    // Keep only the referenced vertices, in first-use order, and renumber the indices.
    static void Compact(VertexData src, uint[] indices, out byte[] bytes, out int count, out ushort[] outIdx)
    {
        int stride = src.VertexStride;
        int n = src.VertexCount;
        var map = new int[n];
        for (int i = 0; i < n; i++) map[i] = -1;
        int next = 0;
        outIdx = new ushort[indices.Length];
        for (int i = 0; i < indices.Length; i++)
        {
            int v = (int)indices[i];
            if (v < 0 || v >= n) throw new Exception("index " + v + " outside " + n + " vertices");
            if (map[v] < 0) map[v] = next++;
            outIdx[i] = (ushort)map[v];
        }
        if (next > 65535) throw new Exception("more than 65535 vertices after compaction");
        bytes = new byte[next * stride];
        for (int v = 0; v < n; v++)
            if (map[v] >= 0) Buffer.BlockCopy(src.VertexBytes, v * stride, bytes, map[v] * stride, stride);
        count = next;
    }

    static DrawableModel CloneShell(DrawableModel s)
    {
        var m = new DrawableModel();
        m.SkeletonBinding = s.SkeletonBinding;
        m.RenderMaskFlags = s.RenderMaskFlags;
        m.ShaderMapping = (ushort[])s.ShaderMapping.Clone();
        m.BoundsData = (AABB_s[])s.BoundsData.Clone();
        m.Geometries = new DrawableGeometry[s.Geometries.Length];
        m.GeometriesCount1 = m.GeometriesCount2 = m.GeometriesCount3 = (ushort)s.Geometries.Length;
        return m;
    }

    // A LOD geometry: same shader, bounds and layout as the high one, own compacted buffers.
    static DrawableGeometry CloneGeometry(DrawableGeometry s, uint[] indices)
    {
        byte[] bytes; int count; ushort[] idx;
        Compact(s.VertexData, indices, out bytes, out count, out idx);

        var vd = new VertexData();
        vd.VertexStride = s.VertexData.VertexStride;
        vd.VertexCount = count;
        vd.Info = s.VertexData.Info;
        vd.VertexType = s.VertexData.VertexType;
        vd.VertexBytes = bytes;

        var vb = new VertexBuffer();
        vb.VertexStride = s.VertexBuffer.VertexStride;
        vb.Flags = s.VertexBuffer.Flags;
        vb.Info = s.VertexBuffer.Info;
        vb.Data1 = vd;
        vb.Data2 = vd;

        var ib = new IndexBuffer();
        ib.Indices = idx;
        ib.IndicesCount = (uint)idx.Length;

        var g = new DrawableGeometry();
        g.VertexBuffer = vb;
        g.IndexBuffer = ib;
        g.VertexData = vd;
        g.ShaderID = s.ShaderID;
        g.Shader = s.Shader;
        g.AABB = s.AABB;
        g.BoneIds = s.BoneIds != null ? (ushort[])s.BoneIds.Clone() : null;
        g.IndicesCount = ib.IndicesCount;
        g.TrianglesCount = ib.IndicesCount / 3;
        g.VerticesCount = (ushort)count;
        g.VertexStride = vb.VertexStride;
        return g;
    }

    static void Apply(YftFile yft, string planPath)
    {
        var drawables = Drawables(yft.Fragment);
        var plan = ReadPlan(planPath);

        // Pass 1: new LODs and edits to the high LOD all read the ORIGINAL high
        // vertex data, so LODs go first and the high LOD is compacted last.
        var touched = new HashSet<int>();
        foreach (var e in plan)
        {
            if (e.Lod == 0) continue;
            var d = drawables[e.Drawable];
            var high = d.DrawableModels.High;
            var arr = GetLod(d, e.Lod);
            if (arr == null)
            {
                arr = new DrawableModel[high.Length];
                for (int m = 0; m < high.Length; m++) arr[m] = CloneShell(high[m]);
                SetLod(d, e.Lod, arr);
            }
            arr[e.Model].Geometries[e.Geometry] = CloneGeometry(high[e.Model].Geometries[e.Geometry], e.Indices);
            touched.Add(e.Drawable);
        }

        foreach (var e in plan)
        {
            if (e.Lod != 0) continue;
            var g = drawables[e.Drawable].DrawableModels.High[e.Model].Geometries[e.Geometry];
            byte[] bytes; int count; ushort[] idx;
            Compact(g.VertexData, e.Indices, out bytes, out count, out idx);
            g.VertexData.VertexBytes = bytes;
            g.VertexData.VertexCount = count;
            g.VertexBuffer.VertexCount = (uint)count;
            g.IndexBuffer.Indices = idx;
            g.IndexBuffer.IndicesCount = (uint)idx.Length;
            g.IndicesCount = (uint)idx.Length;
            g.TrianglesCount = (uint)idx.Length / 3;
            g.VerticesCount = (ushort)count;
            touched.Add(e.Drawable);
        }

        foreach (int i in touched)
        {
            var d = drawables[i];
            // a new LOD level has to be reachable at some distance: keep the high
            // distance for the high LOD and spread the rest, unless already set
            for (int lod = 1; lod <= 3; lod++)
            {
                var arr = GetLod(d, lod);
                if (arr == null) continue;
                foreach (var m in arr)
                    foreach (var g in m.Geometries)
                        if (g == null) throw new Exception("drawable " + i + " lod " + lod + " has a geometry without plan entry");
            }
            d.BuildRenderMasks();
            d.BuildAllModels();
            d.BuildVertexDecls();
        }
    }
}
