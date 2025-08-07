MEMORY {
    FLASH : ORIGIN = 0x10000000, LENGTH = 2048K - 4k
    DATA  : ORIGIN = 0x101FF000, LENGTH = 4k
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K + 8K
}


/* ### Boot ROM info
 *
 * Goes after .vector_table, to keep it in the first 4K of flash where the Boot ROM (and picotool) can find it
 *
 * We also move .text to start /after/ the boot info
 */
SECTIONS {   
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH
} INSERT AFTER .vector_table;
_stext = ADDR(.start_block) + SIZEOF(.start_block);


/* ### Picotool 'Binary Info' Entries
 *
 * Picotool looks through this block (as we have pointers to it in our header) to find interesting information.
 */
SECTIONS {
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;


/* ### Persistent user data section
 *
 * Is used to store the application user config
 */
SECTIONS {
    .userdata (NOLOAD) :
    {
        KEEP(*(.userdata));
        . = ALIGN(4);
    } > DATA
}


/* ### Boot ROM extra info
 *
 * Goes after everything in our program, so it can contain a signature.
 */
SECTIONS {
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
        __flash_binary_end = .;
    } > FLASH

} INSERT AFTER .uninit;
PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);
