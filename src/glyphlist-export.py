glyphlist = []
glyphs_seen = {}
def read_glyphs(name):
    f = open(name)
    lines = f.readlines()
    import re
    for l in lines:
        if l[0] == '#' or l[0] == '\n':
            continue
        split = re.split('[; ,]+', l)
        name = split[0]
        val = int(split[1], 16)
        if val > 0xffff:
            val = int(split[-1], 16)
        if val == 0xf766 and name != "Fsmall":
            continue
        if name in glyphs_seen:
            continue
        glyphs_seen[name] = True
        glyphlist.append((name,val))
read_glyphs("glyphlist-extended.txt")
read_glyphs("texglyphlist.txt")
read_glyphs("additional.txt")
# there are some conflicts between these files
# e.g. tildewide=0x02dc, vs tildewide=0x0303
# for now we just ignore the subsequent ones
glyphlist.append(('mapsto', 0x21A6))
glyphlist = list(set(glyphlist))
glyphlist.sort()
print "/* Autogenerated from:"
print "     https://github.com/michal-h21/htfgen/commits/master/glyphlist-extended.txt"
print "     https://github.com/2ion/lcdf-typetools/blob/master/texglyphlist.txt"
print "     https://github.com/apache/pdfbox/blob/trunk/pdfbox/src/main/resources/org/apache/pdfbox/resources/glyphlist/additional.txt"
print " */"
print "pub fn name_to_unicode(name: &str) -> Option<u16> {"
print "    let names = ["
print ",\n".join('(\"%s\", 0x%04x)' % (g[0], g[1]) for g in glyphlist)
print "    ];"
print "    let result = names.binary_search_by_key(&name, |&(name,_code)| &name);"
print "    result.ok().map(|indx| names[indx].1)"
print "}"