" hi TNormal ctermfg=249 ctermbg=NONE guifg=#b2b2b2 guibg=NONE
let s:normal_ctermfg = clap#themes#extract_or('Normal', 'fg', 'cterm', '249')
let s:normal_guifg = clap#themes#extract_or('Normal', 'fg', 'gui', '249')
execute 'hi TNormal' 'ctermfg='.s:normal_ctermfg 'guifg='.s:normal_guifg 'ctermbg=NONE guibg=NONE'
execute 'syntax match ClapFile' '/^.*/' 'contains='.join(clap#icon#add_head_hl_groups(), ',')

hi default link ClapFile TNormal
