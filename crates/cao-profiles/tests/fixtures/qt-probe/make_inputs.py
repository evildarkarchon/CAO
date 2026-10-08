"""Writes the hand-edited INI inputs for the Qt reader differential, byte for byte."""

import pathlib
import sys

out = pathlib.Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

HAND_EDITED = r"""; leading comment line
   ; indented comment
rootBefore=1
[general]
rootAfter = 2<SP>
[%general]
realGeneral=3
[Values] ; comment after header
plain=hello
spaced =   padded value<SP><SP><SP>
quotedSpaces="  keep  "
quotedConcat="a" b "c"
quotedComma="1,2"
quotedSemicolon="x;y" ; trailing comment
inlineComment=before;after
escapes=\a\b\f\n\r\t\v\"\?\'\\
hexGreedy=\x41g
hexLong=\x0041
hexOverflow=\x12345
hexNoDigits=a\xg
octal=\101\0end
octalStop=\18
unknownEscape=a\qb
continued=one\
two
multiLineQuoted="line1
line2"
latin1=caf@E9@
atAt=@@literal
atString=@String(inner)
atUnknown=@Foo(bar)
invalid=@Invalid()
dup=first
DUP=second
[Lists]
ints=85, 86, 115
quotedElement=a, "b, c", d
trailingComma=a,
spacesAround= x ,  y<SP><SP>
variantPath=@Invalid(), 5
atAtElement=@@a, b
variant85=@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x2\0\0\0U)
variantUInt=@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x3\0\0\0\x62)
variantTwo=@Variant(\0\0\0\t\0\0\0\x2\0\0\0\x2\0\0\0U\0\0\0\x2\0\0\0V)
variantString=@Variant(\0\0\0\t\0\0\0\x1\0\0\0\n\0\0\0\x4\0\x39\0\x38)
variantTruncated=@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x2\0\0)
[Numbers]
spacedInt= 42<SP>
plusInt=+7
negativeInt=-7
negativeUInt=-1
wrapInt=4294967296
wrapUInt=4294967297
fraction=1.5
word=abc
double=2e+09
spacedDouble= 3.5<SP>
[Bools]
upper=TRUE
mixed=False
no=no
zero=0
one=1
empty=
spaceFalse= false
[BSA]
first=1
[bsa]
second=2
[Sub]
a\b=1
%41key=2
%U00e9=3
bad%zz=4
trailing%=5
key with spaces = 6
"""

# `@E9@` marks a raw 0xE9 byte. It makes the file invalid UTF-8, so both Qt and the
# Rust port (deviation 14) decode it as Latin-1.
# `<SP>` marks a trailing space that an editor would otherwise strip.
data = HAND_EDITED.replace("<SP>", " ").replace("\n", "\r\n").encode("ascii").replace(b"@E9@", b"\xe9")
(out / "hand-edited.ini").write_bytes(data)

(out / "lf-only.ini").write_bytes(
    b"[Textures]\ntexturesFormat=98\ntexturesUnwantedFormats=86, 85\ncontinued=a\\\nb\n"
)
(out / "missing-equals.ini").write_bytes(
    b"[BSA]\r\nbsaGame=4\r\nnot a key\r\nbsaEnabled=true\r\n"
)
(out / "unclosed-section.ini").write_bytes(b"[BSA\r\nbsaGame=4\r\n")

# No UTF-8 BOM input here: Qt 5.15 keeps the BOM bytes in its root section, so a BOM
# file reads with FormatError and a header-less first key gains an `ï»¿` prefix. The
# Rust port skips the BOM instead, which its own tests pin.
