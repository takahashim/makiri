# frozen_string_literal: true

# Pure-Ruby reader for the export-name table of a PE32/PE32+ DLL.
#
# Used by the Windows branch of `rake symbols`. External tools (nm/objdump)
# are unreliable on the GitHub Actions Windows runners: some are missing,
# others ship incompatible output formats, and parsing them has proven fragile.
# Reading the PE export directory directly removes that dependency.
module MakiriBuild
  class PEExports
    DOS_LFANEW          = 0x3c
    PE_SIG              = "PE\0\0"
    COFF_SIZE           = 20
    OPTIONAL_HDR_MAGIC  = {
      pe32: 0x10b,
      pe32_plus: 0x20b,
    }.freeze
    EXPORT_DIR_DD_OFFSET = {
      pe32: 96,
      pe32_plus: 112,
    }.freeze
    SECTION_HDR_SIZE    = 40
    EXPORT_DIR_SIZE     = 40
    NAME_POINTER_SIZE   = 4

    def initialize(path)
      @path = path
    end

    # Returns an array of exported symbol names, or [] on any error.
    def names
      File.open(@path, "rb") do |f|
        read_from(f)
      end
    rescue StandardError => e
      warn "PEExports(#{@path}) failed: #{e.class}: #{e.message}"
      []
    end

    private

    def read_from(f)
      pe_offset = read_dword(f, DOS_LFANEW)
      return [] unless read_bytes(f, pe_offset, PE_SIG.bytesize) == PE_SIG

      coff = read_bytes(f, pe_offset + PE_SIG.bytesize, COFF_SIZE)&.unpack("vvVVVVvv")
      return [] unless coff
      nsections = coff[1]
      opt_size  = coff[5]

      opt_start = pe_offset + PE_SIG.bytesize + COFF_SIZE
      magic = read_word(f, opt_start)
      kind = OPTIONAL_HDR_MAGIC.key(magic)
      return [] unless kind

      dd_offset = opt_start + EXPORT_DIR_DD_OFFSET[kind]
      export_rva, export_size = read_bytes(f, dd_offset, 8)&.unpack("VV") || [0, 0]
      return [] if export_rva == 0 || export_size == 0

      sections = read_sections(f, opt_start + opt_size, nsections)
      return [] if sections.size != nsections

      mapper = rva_mapper(sections)
      export_off = mapper.call(export_rva) or return []

      ed = read_bytes(f, export_off, EXPORT_DIR_SIZE)&.unpack("VVVVVVVVVV")
      return [] unless ed
      number_of_names    = ed[6]
      name_pointer_rva   = ed[8]
      return [] if number_of_names == 0

      table_off = mapper.call(name_pointer_rva) or return []
      name_rvas = read_bytes(f, table_off, number_of_names * NAME_POINTER_SIZE)&.unpack("V#{number_of_names}") || []

      name_rvas.filter_map do |name_rva|
        off = mapper.call(name_rva) or next
        read_cstring(f, off)
      end
    end

    def read_sections(f, sec_start, nsections)
      nsections.times.filter_map do |i|
        data = read_bytes(f, sec_start + i * SECTION_HDR_SIZE, SECTION_HDR_SIZE) or next
        _name, virtual_size, virtual_addr, _raw_size, raw_ptr = data.unpack("a8VVVV")
        [virtual_addr, virtual_size, raw_ptr]
      end
    end

    def rva_mapper(sections)
      ->(rva) {
        sections.each do |va, vs, prd|
          return prd + (rva - va) if rva >= va && rva < va + vs
        end
        warn "PEExports(#{@path}): RVA #{rva.to_s(16)} not mapped to file offset"
        nil
      }
    end

    def read_bytes(f, offset, length)
      f.seek(offset)
      data = f.read(length)
      return data if data && data.bytesize == length
      warn "PEExports(#{@path}): short read at #{offset} (wanted #{length}, got #{data&.bytesize || 0})"
      nil
    end

    def read_dword(f, offset)
      read_bytes(f, offset, 4)&.unpack1("V")
    end

    def read_word(f, offset)
      read_bytes(f, offset, 2)&.unpack1("v")
    end

    def read_cstring(f, offset)
      f.seek(offset)
      name = +""
      name << c while (c = f.read(1)) && c != "\0"
      name.empty? ? nil : name
    end
  end
end
