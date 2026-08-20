import struct,sys
TAGS={50708:'UniqueCameraModel',50709:'LocalizedCameraModel',50721:'ColorMatrix1',50722:'ColorMatrix2',
50723:'CameraCalibration1',50724:'CameraCalibration2',50727:'AnalogBalance',50728:'AsShotNeutral',
50778:'CalibrationIlluminant1',50779:'CalibrationIlluminant2',50931:'CameraCalibrationSignature',
50932:'ProfileCalibrationSignature',50933:'ExtraCameraProfiles',50934:'AsShotProfileName',
50936:'ProfileName',50937:'ProfileHueSatMapDims',50938:'ProfileHueSatMapData1',50939:'ProfileHueSatMapData2',
50940:'ProfileToneCurve',50941:'ProfileEmbedPolicy',50942:'ProfileCopyright',50964:'ForwardMatrix1',
50965:'ForwardMatrix2',50981:'ProfileLookTableDims',50982:'ProfileLookTableData',51107:'ProfileHueSatMapEncoding',
51108:'ProfileLookTableEncoding',51109:'BaselineExposureOffset',51110:'DefaultBlackRender',
50730:'BaselineExposure',51111:'NewRawImageDigest',50941:'ProfileEmbedPolicy'}
TYPES={1:('B',1),2:('s',1),3:('H',2),4:('I',4),5:('rat',8),7:('B',1),8:('h',2),9:('i',4),10:('srat',8),11:('f',4),12:('d',8)}
ILLUM={0:'Unknown',1:'Daylight',2:'Fluorescent',3:'Tungsten',17:'StdA',18:'StdB',19:'StdC',20:'D55',21:'D65',22:'D75',23:'D50',24:'ISO studio tungsten',10:'Flash',4:'Flash'}
d=open(sys.argv[1],'rb').read()
assert d[:2]==b'II'; magic=struct.unpack('<H',d[2:4])[0]; off=struct.unpack('<I',d[4:8])[0]
print(f"magic=0x{magic:04X} ifd_offset={off} filesize={len(d)}")
n=struct.unpack('<H',d[off:off+2])[0]; print("entries:",n)
for i in range(n):
    e=off+2+i*12
    tag,typ,cnt=struct.unpack('<HHI',d[e:e+8])
    vo=d[e+8:e+12]
    fmt,sz=TYPES.get(typ,('B',1)); tot=cnt*sz
    if tot<=4: raw=vo[:tot]; dataoff='inline'
    else:
        dataoff=struct.unpack('<I',vo)[0]; raw=d[dataoff:dataoff+tot]
    name=TAGS.get(tag,f'Unknown')
    def rats(r,signed):
        out=[]
        for j in range(0,len(r),8):
            a,b=struct.unpack('<ii' if signed else '<II',r[j:j+8]); out.append(a/b if b else 0.0)
        return out
    if typ==2: val=raw.split(b'\0')[0].decode('utf-8','replace')
    elif typ in (5,10): v=rats(raw,typ==10); val=[round(x,6) for x in v[:12]]+(['...'] if len(v)>12 else [])
    elif typ==3: v=list(struct.unpack('<%dH'%cnt,raw)); val=v[:12]+(['...'] if cnt>12 else [])
    elif typ==4: v=list(struct.unpack('<%dI'%cnt,raw)); val=v[:12]+(['...'] if cnt>12 else [])
    elif typ==11: v=list(struct.unpack('<%df'%cnt,raw)); val=[round(x,5) for x in v[:9]]+(['...'] if cnt>9 else [])
    else: val=raw[:16].hex()+('...' if tot>16 else '')
    if tag in (50778,50779) and typ==3: val=f"{val} -> {ILLUM.get(val[0],'?')}"
    print(f"  {tag:6} 0x{tag:04X} {name:28} type={typ:2} count={cnt:<8} bytes={tot:<9} {val}")
