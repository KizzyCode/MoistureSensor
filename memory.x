MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 256
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 256 - 4k
    DATA  : ORIGIN = 0x101FF000, LENGTH = 4k
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K + 8K
}

EXTERN(BOOT2_FIRMWARE)

SECTIONS {
    /* ### Boot loader */
    .boot2 ORIGIN(BOOT2) :
    {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;

SECTIONS {
    /* Persistent user data section */
    .userdata (NOLOAD) :
    {
        KEEP(*(.userdata));
    } > DATA
}
