/=  *  /common/wrapper
=>
~%  %choo  ..keep  ~
|%
+$  state-0  [%0 *]
+$  state-1  [%1 *]
+$  state-2  [%2 cached-hoon=(unit (trap vase)) *]
+$  state-3  [%3 cached-hoon=(unit (trap vase)) bc=build-cache pc=parse-cache]
+$  versioned-state
  $%  state-0
      state-1
      state-2
      state-3
  ==
+$  choo-state  state-3
::
++  empty-trap-vase
  ^-  (trap vase)
  =>  vaz=!>(~)
  |.(vaz)
::
++  moat  (keep choo-state)
+$  cause
  $%  $:  %build
          pat=cord
          tex=cord
          directory=(list [cord @])
          arbitrary=?
          out=cord
      ==
      $:  %parse
          pat=cord
          tex=cord
          directory=(list [cord @])
      ==
      $:  %prime
          pat=cord
          tex=cord
          directory=(list [cord @])
          native=(list [cord *])
      ==
      [%file %write path=@t contents=@ success=?]
      [%boot hoon-txt=cord]
      [%clear ~]
  ==
+$  cause-debug
  $%  $:  %build
          pat=cord
          tex=cord
          directory=(list [cord @])
          arbitrary=?
          out=cord
      ==
      $:  %parse
          pat=cord
          tex=cord
          directory=(list [cord @])
      ==
      $:  %prime
          pat=*
          tex=*
          directory=*
          native=*
      ==
      [%file %write path=@t contents=@ success=?]
      [%boot hoon-txt=cord]
      [%clear ~]
  ==
+$  effect
  $%  [%file %write path=@t contents=@]
      [%exit id=@]
  ==
::
::
::  hash of file contents
+$  file-hash  @uvI
::
::  hash of file contents along with the merk-hash of its dependencies
+$  merk-hash  @uvI
+$  build-cache  (map merk-hash (trap vase))
::
::  $parse-cache: hash addressed map of preprocessed hoon files.
+$  parse-cache  (map file-hash [=path pil=pile deps=(list raut)])
::
::  $taut: file import from /lib or /sur
+$  taut  [face=(unit term) pax=term]
::
::  $raut: resolved taut.
::    pax contains real path to file after running +get-fit
+$  raut
  [face=(unit @tas) pax=path]
::
::  $pile:  preprocessed hoon file
::
+$  pile
  $:  sur=(list taut)  ::  /-
      lib=(list taut)  ::  /+
      raw=(list [face=(unit term) pax=path])  ::  /=
      bar=(list [face=term mark=@tas =path])  ::  /*
      hax=(list taut)                         ::  /#
      =hoon
  ==
::
+$  octs  [p=@ud q=@]
::
::  $node: entry of adjacency matrix with metadata
::
+$  node
  $:  =path
      hash=@uvI           ::  hash of path contents, not to be confused with merkle dag hash
      deps=(list raut)    ::  holds only outgoing edges
      leaf=graph-leaf
      eval=?              :: whether or not to kick it
  ==
::
+$  graph-leaf
  $%  [%hoon =hoon]
      [%octs =octs]
  ==
::
::
::  $merk-dag: content-addressed map of nodes
::
::    maps content hashes to nodes. each hash is computed from the node's
::    content and the hashes of its dependencies, forming a merkle tree.
::    used to detect changes in the dependency graph and enable caching.
::
+$  merk-dag  [merk=(map merk-hash node) file=(map file-hash merk-hash)]
::
::
::  $graph-view: adjacency matrix with easier access to neighbors
::
::    used to keep track of traversal when building the merkle DAG
::
+$  graph-view  (map path (set path))
--
::
=<
%-  (moat &)
^-  fort:moat
|_  k=choo-state
+*  builder  +>
::
::  +load: upgrade from previous state
::
++  load
  |=  old=versioned-state
  ^-  choo-state
  ::
  ::  We do not use the result of the soft because
  ::  clamming (trap vase) overwrites the contents
  ::  with the bunt resulting in the honc and the build
  ::  artifacts being replaced with empty-trap-vase.
  ::
  ?~  ((soft versioned-state) old)
    ~>  %slog.[0 leaf+"choo: +load old state does not nest under versioned-state. Try booting with --new to start from scratch."]
    !!
  ?-    -.old
      %0
    ~>  %slog.[0 leaf+"update 0-to-2, starting from scratch"]
    *choo-state
  ::
      %1
    ~>  %slog.[0 leaf+"update 1-to-2, starting from scratch"]
    *choo-state
  ::
      %2
    ~>  %slog.[0 leaf+"update 2-to-3, erasing caches but keeping honc"]
    %*  .  *choo-state
      cached-hoon  cached-hoon.old
    ==
  ::
      %3
    ~>  %slog.[0 leaf+"no upgrade"]
    old
  ==
::
::  +peek: external inspect
::
++  peek
  |=  =path
  ^-  (unit (unit *))
  ?+    path  ~
      [%booted ~]
    ``?=(^ cached-hoon.k)
  ==
::
::  +poke: external apply
::
++  poke
  |=  [=wire eny=@ our=@ux now=@da dat=*]
  ^-  [(list effect) choo-state]
  =/  cause=(unit cause)  ((soft cause) dat)
  ?~  cause
    =/  debug=(unit cause-debug)  ((soft cause-debug) dat)
    ?~  debug
      =+  scratch=(debug-hoon-noun dat)
      ~&  "hoonc: warning: input is not a proper cause"
      !!
    =/  debug  u.debug
    ?:  ?=(%prime -.debug)
      =+  scratch=(debug-prime pat.debug tex.debug directory.debug native.debug)
      ~&  "hoonc: warning: %prime cause did not mold, dumping native entries"
      !!
    ~&  "hoonc: warning: input is not a proper cause"
    !!
  =/  cause  u.cause
  ?-    -.cause
      %file
    ?:  success.cause
      ~&  "hoonc: output written successfully to {<path.cause>}"
      [[%exit 0]~ k]
    ~&  "hoonc: failed to write output to {<path.cause>}"
    [[%exit 1]~ k]
  ::
      %clear
    [[%exit 0]~ k(pc *parse-cache, bc *build-cache)]
  ::
      %boot
    =/  cached=?  ?=(^ cached-hoon.k)
    ~&  >>  [hoon-version+hoon-version cached+cached]
    ?:  cached
      [~ k]
   [~ k(cached-hoon `(build-honc hoon-txt.cause))]
  :::
      %build
    =/  target-path=path  (parse-file-path pat.cause)
    ::
    ::  Create map of dep directory, includes target
    =/  dir
      %-  ~(gas by *(map path @))
      :-  [target-path tex.cause]
      (turn directory.cause |=((pair @t @) [(stab p) q]))
    ?>  ?=(^ cached-hoon.k)
    =/  [compiled=(unit *) new-bc=build-cache new-pc=parse-cache]
      %-  ~(create builder u.cached-hoon.k bc.k pc.k)
      [target-path dir arbitrary.cause]
    :_  k(bc new-bc, pc new-pc)
    ?~  compiled
      ~&  "hoonc: build failed, skipping write and exiting"
      [%exit 1]~
    ~&  "hoonc: build succeeded, sending out write effect"
    [%file %write path=out.cause contents=(jam u.compiled)]~
  ::
      %parse
    =/  target-path=path  (parse-file-path pat.cause)
    ::
    ::  Create map of dep directory, includes target
    =/  dir
      %-  ~(gas by *(map path @))
      :-  [target-path tex.cause]
      (turn directory.cause |=((pair @t @) [(stab p) q]))
    =/  parse-res  (parse-dir dir)
    =/  new-pc=parse-cache  +.parse-res
    [~ k(pc new-pc)]
  ::
      %prime
    =/  target-path=path  (parse-file-path pat.cause)
    ::
    ::  Create map of dep directory, includes target
    =/  dir
      %-  ~(gas by *(map path @))
      :-  [target-path tex.cause]
      (turn directory.cause |=((pair @t @) [(stab p) q]))
    =/  native-map=(map path hoon)
      (prime-native-map native.cause)
    =/  new-pc=parse-cache  (prime-dir target-path dir native-map)
    [~ k(pc new-pc)]
  ==
--
=>
::
::  dependency system
::
~%  %dependency-system  +  ~
|%
::  debug helpers
++  debug-prime
  |=  [pat=* tex=* directory=* native=*]
  ^-  ?
  =/  patu  ((soft cord) pat)
  ?~  patu
    =+  scratch=(debug-hoon-noun pat)
    ~&  "hoonc: prime-debug pat is not a cord"
    %.y
  =/  texu  ((soft cord) tex)
  ?~  texu
    =+  scratch=(debug-hoon-noun tex)
    ~&  "hoonc: prime-debug tex is not a cord"
    %.y
  =/  listu  ((soft (list *)) directory)
  ?~  listu
    ~&  "hoonc: prime-debug directory is not a list"
    %.y
  =/  pairu  ((soft (list [* *])) directory)
  ?~  pairu
    ~&  "hoonc: prime-debug directory list entries are not pairs"
    %.y
  =/  diru  ((soft (list [cord @])) directory)
  ?~  diru
    =+  scratch=(debug-directory directory)
    ~&  "hoonc: prime-debug directory is not (list [cord @])"
    %.y
  =+  scratch=(debug-native native)
  scratch
::
++  debug-native
  |=  native=*
  ^-  ?
  =/  native-mug  (mug native)
  ~&  "hoonc: prime-debug native mug {<native-mug>}"
  ?@  native
    ~&  "hoonc: prime-debug native is atom"
    %.n
  =/  head  -.native
  =/  tail  +.native
  ~&  "hoonc: prime-debug native head mug {<(mug head)>}"
  ~&  "hoonc: prime-debug native tail mug {<(mug tail)>}"
  ?@  head
    ~&  "hoonc: prime-debug native head is atom"
    %.n
  =/  path  -.head
  =/  raw  +.head
  ~&  "hoonc: prime-debug entry path mug {<(mug path)>}"
  ~&  "hoonc: prime-debug entry raw mug {<(mug raw)>}"
  =+  scratch=(debug-hoon-noun head)
  %.n
::
++  debug-directory
  |=  dir=*
  ^-  ?
  =/  idx=@ud  0
  |-
    ?@  dir
      ?:  =(dir 0)
        ~&  "hoonc: prime-debug directory ended after {<idx>} entries"
        %.n
      ~&  "hoonc: prime-debug directory tail atom {<dir>} at index {<idx>}"
      %.y
    =/  entry  -.dir
    =/  rest  +.dir
    ?@  entry
      ~&  "hoonc: prime-debug directory entry atom at index {<idx>}"
      %.y
    ?:  ?=([cord @] entry)
      $(dir rest, idx +(idx))
    ~&  "hoonc: prime-debug directory entry {<idx>} does not mold as [cord @]"
    ~&  "hoonc: prime-debug directory entry mug {<(mug entry)>}"
    =/  key  -.entry
    =/  val  +.entry
    ?@  key
      ?@  val
        =/  keyu  ((soft cord) key)
        ?~  keyu
          ~&  "hoonc: prime-debug directory entry {<idx>} key is not cord, mug {<(mug key)>}"
          %.y
        ~&  "hoonc: prime-debug directory entry {<idx>} val is atom for path {<u.keyu>}"
        %.y
      ~&  "hoonc: prime-debug directory entry {<idx>} val is cell, mug {<(mug val)>}"
      %.y
    ~&  "hoonc: prime-debug directory entry {<idx>} key is cell, mug {<(mug key)>}"
    %.y
::
++  debug-hoon-noun
  |=  raw=*
  ^-  *
  =/  raw-mug  (mug raw)
  ~&  "hoonc: prime-debug noun mug {<raw-mug>}"
  ?@  raw
    ~&  "hoonc: prime-debug noun is atom {<raw>}"
    ~
  =/  head  -.raw
  =/  tail  +.raw
  ?@  head
    ~&  "hoonc: prime-debug head is atom"
    =/  head-term=(unit term)  ((soft term) head)
    ?^  head-term
      ~&  "hoonc: prime-debug head term {<(scow %tas u.head-term)>}"
      ?:  =(u.head-term %tssg)
        =/  idx=@ud  0
        =/  items  tail
        |-
          ?@  items
            ?:  =(items 0)
              ~
            ~&  "hoonc: prime-debug tssg list tail atom {<items>} at index {<idx>}"
            ~
          =/  item  -.items
          =/  rest  +.items
          ?~  ((soft hoon) item)
            ~&  "hoonc: prime-debug tssg item {<idx>} failed hoon mold"
            =+  scratch=(debug-hoon-noun item)
            ~
          $(items rest, idx +(idx))
      ?:  =(u.head-term %tsgr)
        ?@  tail
          ~&  "hoonc: prime-debug tsgr tail atom {<tail>}"
          ~
        =/  left  -.tail
        =/  right  +.tail
        =/  left-ok  ((soft hoon) left)
        ?~  left-ok
          ~&  "hoonc: prime-debug tsgr left failed hoon mold"
          =+  scratch=(debug-hoon-noun left)
          ~
        =/  right-ok  ((soft hoon) right)
        ?~  right-ok
          ~&  "hoonc: prime-debug tsgr right failed hoon mold"
          =+  scratch=(debug-hoon-noun right)
          ~
        ~
      ?:  =(u.head-term %brcn)
        ?@  tail
          ~&  "hoonc: prime-debug brcn tail atom {<tail>}"
          ~
        =/  prefix  -.tail
        =/  tomes  +.tail
        =/  prefix-ok  ((soft (unit term)) prefix)
        ?~  prefix-ok
          ~&  "hoonc: prime-debug brcn prefix failed unit term"
          ~
        ?:  (debug-map-term-tome tomes)
          ~&  "hoonc: prime-debug brcn tomes map failed"
          ~
        ~
      ?:  =(u.head-term %dbug)
        ?@  tail
          ~&  "hoonc: prime-debug dbug tail atom {<tail>}"
          ~
        =/  spot  -.tail
        =/  inner  +.tail
        =/  spot-ok  ((soft spot) spot)
        ?~  spot-ok
          ~&  "hoonc: prime-debug dbug spot failed"
          ~
        =/  inner-ok  ((soft hoon) inner)
        ?~  inner-ok
          ~&  "hoonc: prime-debug dbug inner failed hoon mold"
          =+  scratch=(debug-hoon-noun inner)
          ~
        ~
      ~
    ~&  "hoonc: prime-debug head atom {<head>}"
    ~&  "hoonc: prime-debug head mug {<(mug head)>}"
    ~&  "hoonc: prime-debug tail mug {<(mug tail)>}"
    ~
  ~&  "hoonc: prime-debug head is cell"
  ~&  "hoonc: prime-debug head head mug {<(mug -.head)>}"
  ~&  "hoonc: prime-debug head tail mug {<(mug +.head)>}"
  ~&  "hoonc: prime-debug tail mug {<(mug tail)>}"
  ~

++  find-noun-mismatch
  |=  [a=* b=* path=*]
  ^-  (unit [path=* a=* b=*])
  ?:  =(a b)
    ~
  ?@  a
    [~ [path a b]]
  ?@  b
    [~ [path a b]]
  =/  head=(unit [path=* a=* b=*])
    (find-noun-mismatch -.a -.b (snoc path 0))
  ?^  head
    head
  (find-noun-mismatch +.a +.b (snoc path 1))

++  log-noun-mismatch
  |=  [path=* a=* b=*]
  ^-  *
  ~&  "prime-diff path {<path>}"
  ~&  "prime-diff mug a {<(mug a)>} b {<(mug b)>}"
  ~

++  strip-dbug
  |=  gen=*
  ^-  *
  ?@  gen  gen
  =/  head  -.gen
  =/  tail  +.gen
  ?:  ?=(%dbug head)
    ?@  tail  gen
    $(gen +.tail)
  :*  $(gen head)
      $(gen tail)
  ==

::  +debug-map-term-tome: validate map term->tome
++  debug-map-term-tome
  |=  map=*
  ^-  ?
  ?@  map
    ?:  =(map 0)
      %.n
    ~&  "hoonc: prime-debug map term->tome tail atom {<map>}"
    %.y
  =/  node  -.map
  =/  kids  +.map
  ?@  node
    ~&  "hoonc: prime-debug map term->tome node atom {<node>}"
    %.y
  =/  key  -.node
  =/  val  +.node
  =/  key-ok  ((soft term) key)
  ?~  key-ok
    ~&  "hoonc: prime-debug map term->tome key not term {<key>}"
    %.y
  ?:  (debug-tome-noun val)
    ~&  "hoonc: prime-debug map term->tome value failed for {<(scow %tas u.key-ok)>}"
    %.y
  =/  left  -.kids
  =/  right  +.kids
  ?:  (debug-map-term-tome left)
    %.y
  (debug-map-term-tome right)
::  +debug-map-term-hoon: validate map term->hoon
++  debug-map-term-hoon
  |=  map=*
  ^-  ?
  ?@  map
    ?:  =(map 0)
      %.n
    ~&  "hoonc: prime-debug map term->hoon tail atom {<map>}"
    %.y
  =/  node  -.map
  =/  kids  +.map
  ?@  node
    ~&  "hoonc: prime-debug map term->hoon node atom {<node>}"
    %.y
  =/  key  -.node
  =/  val  +.node
  =/  key-ok  ((soft term) key)
  ?~  key-ok
    ~&  "hoonc: prime-debug map term->hoon key not term {<key>}"
    %.y
  =/  val-ok  ((soft hoon) val)
  ?~  val-ok
    ~&  "hoonc: prime-debug map term->hoon value failed for {<(scow %tas u.key-ok)>}"
    =+  scratch=(debug-hoon-noun val)
    %.y
  =/  left  -.kids
  =/  right  +.kids
  ?:  (debug-map-term-hoon left)
    %.y
  (debug-map-term-hoon right)
::  +debug-tome-noun: validate tome structure
++  debug-tome-noun
  |=  raw=*
  ^-  ?
  ?@  raw
    ~&  "hoonc: prime-debug tome atom {<raw>}"
    %.y
  =/  wot  -.raw
  =/  arms  +.raw
  =/  what-ok  ((soft what) wot)
  ?~  what-ok
    ~&  "hoonc: prime-debug tome what failed mold"
    %.y
  ?:  (debug-map-term-hoon arms)
    %.y
  %.n
::  +parse-file-path: parse cord of earth file path to $path
++  parse-file-path
  |=  pat=cord
  (rash pat gawp)
::
::  +gawp: parse an absolute earth file path
++  gawp
  %+  sear
    |=  p=path
    ^-  (unit path)
    ?:  ?=([~ ~] p)  `~
    ?.  =(~ (rear p))  `p
    ~
  ;~(pfix fas (most fas bic))
::
::  +bic: parse file/dir name in earth file path
++  bic
  %+  cook
  |=(a=tape (rap 3 ^-((list @) a)))
  (star ;~(pose nud low hig hep dot sig cab))
::
++  to-wain                                           ::  cord to line list
  |=  txt=cord
  ^-  wain
  ?~  txt  ~
  =/  len=@  (met 3 txt)
  =/  cut  =+(cut -(a 3, c 1, d txt))
  =/  sub  sub
  =|  [i=@ out=wain]
  |-  ^+  out
  =+  |-  ^-  j=@
      ?:  ?|  =(i len)
              =(10 (cut(b i)))
          ==
        i
      $(i +(i))
    =.  out  :_  out
    (cut(b i, c (sub j i)))
  ?:  =(j len)
    (flop out)
  $(i +(j))
::
++  parse-pile
  |=  [pax=path tex=tape]
  ^-  pile
  =/  [=hair res=(unit [=pile =nail])]
    %-  road  |.
    ((pile-rule pax) [1 1] tex)
  ?^  res  pile.u.res
  %-  mean
  =/  lyn  p.hair
  =/  col  q.hair
  ^-  (list tank)
  :~  leaf+"syntax error at [{<lyn>} {<col>}] in {<pax>}"
    ::
      =/  =wain  (to-wain (crip tex))
      ?:  (gth lyn (lent wain))
        '<<end of file>>'
      (snag (dec lyn) wain)
    ::
      leaf+(runt [(dec col) '-'] "^")
  ==
::
++  pile-rule
  |=  pax=path
  %-  full
  %+  ifix
    :_  gay
    ::  parse optional /? and ignore
    ::
    ;~(plug gay (punt ;~(plug fas wut gap dem gap)))
  |^
  ;~  plug
    %+  cook  (bake zing (list (list taut)))
    %+  rune  hep
    (most ;~(plug com gaw) taut-rule)
  ::
    %+  cook  (bake zing (list (list taut)))
    %+  rune  lus
    (most ;~(plug com gaw) taut-rule)
  ::
    %+  rune  tis
    ;~(plug ;~(pose (cold ~ tar) (stag ~ sym)) ;~(pfix gap stap))
  ::
    %+  rune  tar
    ;~  (glue gap)
      sym
      ;~(pfix cen sym)
      ;~(pfix stap)
    ==
  ::
    %+  cook  (bake zing (list (list taut)))
    %+  rune  hax
    (most ;~(plug com gaw) taut-rule)
  ::
    %+  stag  %tssg
    (most gap tall:(vang [& |] pax))
  ==
  ::
  ++  pant
    |*  fel=rule
    ;~(pose fel (easy ~))
  ::
  ++  mast
    |*  [bus=rule fel=rule]
    ;~(sfix (more bus fel) bus)
  ::
  ++  rune
    |*  [bus=rule fel=rule]
    %-  pant
    %+  mast  gap
    ;~(pfix fas bus gap fel)
  --
::
++  taut-rule
  %+  cook  |=(taut +<)
  ;~  pose
    (stag ~ ;~(pfix tar sym))               ::  *foo -> [~ %foo]
    ;~(plug (stag ~ sym) ;~(pfix tis sym))  ::  bar=foo -> [[~ %bar] %foo]
    (cook |=(a=term [`a a]) sym)            ::  foo    -> [[~ %foo] %foo]
  ==
::
++  segments
  |=  suffix=@tas
  ^-  (list path)
  =/  parser
    (most hep (cook crip ;~(plug ;~(pose low nud) (star ;~(pose low nud)))))
  =/  torn=(list @tas)  (fall (rush suffix parser) ~[suffix])
  %-  flop
  |-  ^-  (list (list @tas))
  ?<  ?=(~ torn)
  ?:  ?=([@ ~] torn)
    ~[torn]
  %-  zing
  %+  turn  $(torn t.torn)
  |=  s=(list @tas)
  ^-  (list (list @tas))
  ?>  ?=(^ s)
  ~[[i.torn s] [(crip "{(trip i.torn)}-{(trip i.s)}") t.s]]
::
++  get-fit
  |=  [pre=@ta pax=@tas dir=(map path @)]
  ^-  (unit path)
  =/  paz=(list path)  (segments pax)
  |-
  ?~  paz
    ~&  >>  "choo: missing dependency {<pax>}"
    ~
  =/  last=term  (rear i.paz)
  =.  i.paz   `path`(snip i.paz)
  =/  puz
    ^-  path
    %+  snoc
      `path`[pre i.paz]
    `@ta`(rap 3 ~[last %'.' %hoon])
  ?^  (~(get by dir) puz)
    `puz
  $(paz t.paz)
::
++  resolve-pile
  ::  turn fits into resolved path suffixes
  |=  [=pile dir=(map path @)]
  ^-  (list raut)
  ;:  weld
    (turn sur.pile |=(taut ^-(raut [face (need (get-fit %sur pax dir))])))
    (turn lib.pile |=(taut ^-(raut [face (need (get-fit %lib pax dir))])))
  ::
    %+  turn  raw.pile
    |=  [face=(unit term) pax=path]
    =/  pax-snip  (snip pax)
    =/  pax-rear  (rear pax)
    ^-  raut
    =/  pat=path  (snoc pax-snip `@ta`(rap 3 ~[pax-rear %'.' %hoon]))
    ?.  (~(has by dir) pat)
      ~&  "hoonc: missing dependency {<pat>}"
      !!
    [face pat]
  ::
    %+  turn  bar.pile
    |=  [face=term mark=@tas pax=path]
    =/  pax-snip  (snip pax)
    =/  pax-hind  (rear pax-snip)
    =/  pax-rear  (rear pax)
    ^-  raut
    =/  pat=path  (snoc (snip pax-snip) `@ta`(rap 3 ~[pax-hind %'.' pax-rear]))
    ?.  (~(has by dir) pat)
      ~&  "hoonc: missing dependency {<pat>}"
      !!
    [`face pat]
  ::
    (turn hax.pile |=(taut ^-(raut [face (need (get-fit %dat pax dir))])))
  ==
--
::
::  builder core
::
~%  %builder  +>  ~
|_  [honc=(trap vase) bc=build-cache pc=parse-cache]
::
++  build-honc
  |=  hoon-txt=cord
  ^-  (trap vase)
  ~&  "Please be patient, compiling hoon for the first time takes a while."
  (swet empty-trap-vase (ream hoon-txt))
::
::
::  $create: build a trap from a hoon/jock file with dependencies
::
::    .tar: the path to build
::    .dir: the directory to get dependencies from
::    .arb: arbitrary flag
::
::    If arb is true, we are building a noun of arbitrary shape.
::
::    If arb is false,we are building a kernel gate that takes a hash
::    of the dependency directory.
::
::    returns a trap, a build-cache, and a parse-cache
++  create
  ~/  %create
  |=  [tar=path dir=(map path @) arb=?]
  ^-  [(unit (trap)) build-cache parse-cache]
  =/  dir-hash  `@uvI`(mug dir)
  ~&  >>  dir-hash+dir-hash
  =/  [tase=(unit (trap vase)) =build-cache =parse-cache]
    (create-target tar dir)
  :_  [build-cache parse-cache]
  ::  build failure, just return the bunted trap
  ?~  tase
    ~
  %-  some
  ::
  ::  If arbitrary, return the trap.
  ?:  arb
   =>  u.tase
   |.(+:^$)
  ::
  ::  Otherwise, defer slam the dir hash into the kernel gate
  ::  +shot calls the kernel gate to tell it the hash of the dependency directory
  =>  %+  shot  u.tase
    =>  d=!>(dir-hash)
    |.(d)
  |.(+:^$)
::
::  $create-target: builds a hoon/jock file with dependencies
::
::    .path: the path to build
::    .dir: the directory to get dependencies from
::
::    returns a trap with the compiled hoon/jock file and the updated caches
++  create-target
  ~/  %create-target
  |=  [tar=path dir=(map path @)]
  ^-  [(unit (trap vase)) build-cache parse-cache]
  =^  all-nodes=(map path node)  pc
    (parse-dir dir)
  =/  dag=merk-dag  (build-merk-dag all-nodes)
  ::
  ::  delete invalid cache entries in bc
  =.  bc
    %+  roll
      ~(tap by bc)
    |=  [[=merk-hash *] bc=_bc]
    ?:  (~(has by merk.dag) merk-hash)
      bc
    (~(del by bc) merk-hash)
  ::
  =^  res=(unit (trap vase))  bc
    (~(try bil [all-nodes dag]) tar)
  ::
  [res bc pc]
::
::
::  $parse-dir: create nodes from dir
::
::    .dir: directory of deps, includes build target
::
::    returns a map of nodes and the updated parse cache
++  parse-dir
  |^
  |=  dir=(map path @)
  ^-  [(map path node) parse-cache]
  =|  nodes=(map path node)
  =|  new-pc=parse-cache
  =/  files=(list path)  ~(tap in ~(key by dir))
  |-
  ?~  files
    [nodes new-pc]
  =^  nod=node  new-pc
    (make-node i.files dir new-pc)
  %=    $
    nodes  (~(put by nodes) i.files nod)
    new-pc     new-pc
    files  t.files
  ==
::
  ++  make-node
    |=  [pat=path dir=(map path @) new-pc=parse-cache]
    ^-  [node parse-cache]
    =/  fil=@  (~(got by dir) pat)
    =/  file-hash  (shax fil)                                  ::  hash dep file
    ?.  (is-hoon pat)
      :_  new-pc
      :*  pat                                           ::  path
          file-hash                                     ::  hash
          ~                                             ::  deps
          [%octs [(met 3 fil) fil]]                     ::  octs
          %.n                                           ::  no kick
      ==
    =/  fil-cord=cord  fil
    =/  tex=tape  (trip fil-cord)
    =/  e=(unit [path pile (list raut)])  (~(get by pc) file-hash)
    =/  cacheable=?  %.y
    =/  [pil=pile deps=(list raut)]
      ?~  e
        ~&  "parsing {<pat>}"
        (process-pile pat tex dir)
      =/  old-path=path  -.u.e
      ?:  =(pat old-path)
        ~&  "reusing parse cache entry for {<pat>}"
        +.u.e
      =/  replace=?  (lth (mug pat) (mug old-path))
      ~&  "parse-dir: hash collision {<file-hash>} new {<pat>} old {<old-path>} len {<(met 3 fil)>}"
      =.  cacheable  replace
      (process-pile pat tex dir)
    =.  new-pc
      ?:  cacheable
        (~(put by new-pc) file-hash [pat pil deps])
      new-pc
    :_  new-pc
    :*  pat                                              ::  path
        file-hash                                        ::  hash
        deps                                             ::  deps
        [%hoon hoon.pil]                                 ::  hoon
        (is-dat pat)                                     ::  whether to eval or not
    ==
  ::
  ++  process-pile
    |=  [pax=path tex=tape dir=(map path @)]
    ^-  [pile (list raut)]
    =/  pil  (parse-pile pax tex)
    [pil (resolve-pile pil dir)]
  --
::  $prime-native-map: validate native hoon nouns, log failures, and build map
++  prime-native-map
  |=  native=(list [cord *])
  ^-  (map path hoon)
  =|  out=(map path hoon)
  =|  total=@ud
  =|  bad=@ud
  |-
  ?~  native
    ~&  "prime-native: ok {<(sub total bad)>} bad {<bad>} total {<total>}"
    out
  =.  total  +(total)
  =/  entry=[cord *]  i.native
  =/  path=path  (stab -.entry)
  =/  cast=(unit hoon)  ((soft hoon) +.entry)
  ?^  cast
    =.  out  (~(put by out) path u.cast)
    $(native t.native)
  =.  bad  +(bad)
  ~&  "prime-native: hoon mold failed for {<path>}"
  =+  scratch=(debug-hoon-noun +.entry)
  $(native t.native)
::  $prime-dir: create parse-cache entries using native hoon ASTs
::
::    .tar: entry path to start traversal
::    .dir: directory of deps, includes build target
::    .native: map of hoon ASTs keyed by path
::    returns updated parse-cache
++  prime-dir
  |^
  |=  [tar=path dir=(map path @) native=(map path hoon)]
  ^-  parse-cache
  =|  pc=parse-cache
  =|  seen=(set path)
  ~&  "prime-dir: starting {<tar>}"
  =^  pc  seen
    (prime-node tar dir native pc seen)
  ~&  "prime-dir: pc-size {<~(wyt by pc)>}"
  pc
  ::
  ++  prime-node
    |=  [pat=path dir=(map path @) native=(map path hoon) pc=parse-cache seen=(set path)]
    ^-  [parse-cache (set path)]
    ?:  (~(has in seen) pat)
      [pc seen]
    =.  seen  (~(put in seen) pat)
    ?.  (is-hoon pat)
      [pc seen]
    =/  fil=@  (~(got by dir) pat)
    =/  file-hash  (shax fil)
    =/  native-hoon=(unit hoon)  (~(get by native) pat)
    =/  e=(unit [path pile (list raut)])  (~(get by pc) file-hash)
    =/  cacheable=?  %.y
    =/  [pil=pile deps=(list raut)]
      ?~  e
        ~&  "prime-dir parsing {<pat>}"
        =/  fil-cord=cord  fil
        =/  tex=tape  (trip fil-cord)
        =/  pil  (parse-pile pat tex)
        [pil (resolve-pile pil dir)]
      =/  old-path=path  -.u.e
      ?:  =(pat old-path)
        +.u.e
      =/  replace=?  (lth (mug pat) (mug old-path))
      ~&  "prime-dir: hash collision {<file-hash>} new {<pat>} old {<old-path>} len {<(met 3 fil)>}"
      =.  cacheable  replace
      =/  fil-cord=cord  fil
      =/  tex=tape  (trip fil-cord)
      =/  pil  (parse-pile pat tex)
      [pil (resolve-pile pil dir)]
    =/  pil=pile
      ?~  native-hoon
        pil
      ~&  "prime-dir: injecting native hoon for {<pat>}"
      =/  parsed-hoon  hoon.pil
      =/  native  u.native-hoon
      =/  parsed-clean  (strip-dbug parsed-hoon)
      =/  native-clean  (strip-dbug native)
      ?:  =(parsed-clean native-clean)
        ?:  =(parsed-hoon native)
          pil(hoon native)
        ~&  "prime-dir: hoon spot mismatch for {<pat>}"
        ~&  "prime-dir: using parsed hoon for {<pat>}"
        pil(hoon parsed-hoon)
      ~&  "prime-dir: hoon mismatch for {<pat>}"
      =/  mismatch=(unit [path=* a=* b=*])
        (find-noun-mismatch parsed-clean native-clean ~)
      ?^  mismatch
        =+  scratch=(log-noun-mismatch path.u.mismatch a.u.mismatch b.u.mismatch)
        ~
      ~&  "prime-dir: hoon mug parser {<(mug parsed-clean)>} native {<(mug native-clean)>}"
      ~&  "prime-dir: parsed hoon (clean)"
      =+  scratch=(debug-hoon-noun parsed-clean)
      ~&  "prime-dir: native hoon (clean)"
      =+  scratch=(debug-hoon-noun native-clean)
      pil(hoon native)
    =.  pc
      ?:  cacheable
        (~(put by pc) file-hash [pat pil deps])
      pc
    ~&  "prime-dir: pc-size now {<~(wyt by pc)>} for {<pat>}"
    |-
    ?~  deps
      [pc seen]
    =^  pc  seen
      (prime-node pax.i.deps dir native pc seen)
    $(deps t.deps, pc pc, seen seen)
  --
::
::  $build-merk-dag: builds a merkle DAG out dependencies + target
::
::    .nodes: the nodes of the dependency graph
::
::    returns a merkle DAG and a path-dag
++  build-merk-dag
  |=  nodes=(map path node)
  |^
  ^-  merk-dag
  ::
  =|  dag=merk-dag
  =/  graph  (build-graph-view nodes)
  =/  next=(map path node)  (update-next nodes graph)
  ::
  ::  traverse via a topological sorting of the DAG using Kahn's algorithm
  |-
  ?:  .=(~ next)
    ?.  .=(~ graph)
      ~|(cycle-detected+~(key by graph) !!)
    dag
  =-
    %=  $
      next   (update-next nodes graph)
      graph  graph
      dag  dag
    ==
  ^-  [graph=(map path (set path)) dag=merk-dag]
  ::
  ::  every node in next is put into file-dag and dep-dag along with
  ::  its hash
  %+  roll
    ~(tap by next)
  |=  [[p=path n=node] graph=_graph dag=_dag]
  =/  =merk-hash  (calculate-hash n dag)
  :+  (update-graph-view graph p)
    (~(put by merk.dag) merk-hash n)
  (~(put by file.dag) hash.n merk-hash)
  ::
  ::  $calculate-hash: calculate the hash of a node
  ::
  ::    .n: the node to calculate the hash of
  ::    .dag: the DAG of the dependency graph
  ::
  ::    returns the hash of the node
  ++  calculate-hash
    |=  [nod=node dag=merk-dag]
    ^-  @
    %+  roll
      deps.nod
    |=  [raut running-hash=_hash.nod]
    =/  dep-file-hash  hash:(~(got by nodes) pax)
    ?~  dep-merk-hash=(~(get by file.dag) dep-file-hash)
      ~&  "hoonc: calculate-hash: Missing entry for {<pax>}"
      !!
    (shax (rep 8 ~[running-hash u.dep-merk-hash]))
  ::
  ::  $build-graph-view: build a graph-view from a node map
  ::
  ::    .nodes: the nodes of the dependency graph
  ::
  ::    returns a graph-view of the dependency graph
  ::
  ++  build-graph-view
    |=  nodes=(map path node)
    ^-  graph-view
    %-  ~(urn by nodes)
    |=  [* n=node]
    %-  silt
    (turn deps.n |=(raut pax))
  ::
  ::  $update-graph-view: updates a $graph-view by removing a $path
  ::
  ::    .gv: the graph-view to update
  ::    .p: the path to remove from the graph-view
  ::
  ::    deletes the $path from the $graph-view and removes it from all edge sets
  ::
  ++  update-graph-view
    |=  [gv=graph-view p=path]
    ^-  graph-view
    =.  gv  (~(del by gv) p)
    %-  ~(urn by gv)
    |=  [pax=path edges=(set path)]
    (~(del in edges) p)
  ::
  ::  $update-next: returns nodes from a $graph-view that have no outgoing edges
  ::
  ::    .nodes: the nodes of the dependency graph
  ::    .gv: the graph-view of the dependency graph
  ::
  ::    assumes that entries in $nodes that are not in the $graph-view have
  ::    already been visited.
  ::
  ++  update-next
    |=  [nodes=(map path node) gv=graph-view]
    ^-  (map path node)
    ::
    ::  if we don't have the path in gv, already visited
    %+  roll
      ~(tap by gv)
    |=  [[pax=path edges=(set path)] next=(map path node)]
    ::
    :: if a node has no out edges, add it to next
    ?.  =(*(set path) edges)
      next
    %+  ~(put by next)
      pax
    (~(got by nodes) pax)
  --
::
::
::  Builder core
++  bil
  ~%  %bil  +>  ~
  |_  $:  nodes=(map path node)
          dag=merk-dag
      ==
  ::
  ++  try
    ~/  %try
    |=  tar=path
    ^-  [(unit (trap vase)) build-cache]
    ~+
    =/  nod=node  (~(got by nodes) tar)
    ?^  target=(grab hash.nod)
      ~&  "reusing build cache entry for: {<tar>}"
      [target bc]
    ::
    ::  recursively build dependencies of node
    =^  dep-vaz=(unit (trap vase))  bc
      =+  [dep-vaz=empty-trap-vase deps=`(list raut)`deps.nod]
      |-
      ?~  deps
        [`dep-vaz bc]
      ::
      =^  vaz=(unit (trap vase))  bc
        (try pax.i.deps)
      ::
      ::  Exit trap early due to build failure
      ?~  vaz
        [~ bc]
      ::
      ::  append build dependecy to dep-vaz and put it in the build cache
      %=  $
        dep-vaz  (slat dep-vaz (label-vase u.vaz face.i.deps))
        bc       bc
        deps     t.deps
      ==
    ::
    ::  If one of the deps did not build succesfully, we need to
    ::  Propagate failure back up to caller.
    ::
    ?~  dep-vaz
      [~ bc]
    ?~  target-vaz=(compile nod u.dep-vaz)
      [~ bc]
    ::
    ::  get merk hash of dependency, this is the key to the build cache
    =/  =merk-hash  (~(got by file.dag) hash.nod)
    [target-vaz (~(put by bc) merk-hash u.target-vaz)]
  ::
  ++  compile
    ~/  %compile
    |=  [nod=node dep-vaz=(trap vase)]
    ^-  (unit (trap vase))
    =+  nod-path=path.nod
    =+  nod-leaf=leaf.nod
    =;  result=(each (trap vase) tang)
      ?-  -.result
        %&  `p.result
        %|  ~&  "hoonc: compile failed for {<nod-path>}"
            ~&  "hoonc: compile failed leaf mug {<(mug nod-leaf)>}"
            ~&  (slog p.result)
            ~&  (debug-hoon-noun nod-leaf)
            ~
      ==
    %-  mule
    |.
    ?.  ?=(%hoon -.leaf.nod)
      =>  octs=!>(octs.leaf.nod)
      |.(octs)
    ~>  %bout
    ~&  "compiling {<path.nod>}"
    ::
    ::  Faces are resolved via depth-first search into the subject.
    ::  We append the honc (hoon.hoon) to the end of the vase
    ::  because imports have higher precedence when resolving faces.
    ::  To avoid shadowing issues with hoon.hoon, attach faces to your
    ::  imports or avoid shadowed names altogether.
    =/  swetted=(trap vase)  (swet (slat dep-vaz honc) hoon.leaf.nod)
    ?.  eval.nod
      swetted
    ~&  "node {<path.nod>} is eval, kicking"
    =>  [swetted=swetted vase=vase]
    =/  vaz=vase  $:swetted
    =>  vaz=vaz
    |.(vaz)
  ::
++  grab
  |=  =file-hash
  ^-  (unit (trap vase))
  =/  merk-hash=(unit merk-hash)  (~(get by file.dag) file-hash)
  ?~  merk-hash
    =/  matches=(list path)
      %+  roll
        ~(tap by nodes)
      |=  [[p=path n=node] acc=(list path)]
      ?:  =(hash.n file-hash)
        [p acc]
      acc
    ~&  "hoonc: grab missing file-hash {<file-hash>} file-dag-size {<~(wyt by file.dag)>} matches {<matches>}"
    ~
  (~(get by bc) u.merk-hash)
  --
::
::  $label-vase: label a (trap vase) with a face
::
::    .vaz: the (trap vase) to label
::    .face: the face to label the (trap vase) with
::
::    returns a (trap vase) labeled with the given face
++  label-vase
  |=  [vaz=(trap vase) face=(unit @tas)]
  ^-  (trap vase)
  ?~  face  vaz
  =>  [vaz=vaz face=u.face]
  |.
  =/  vas  $:vaz
  [[%face face p.vas] q.vas]
::
::  $slat: merge two (trap vase)s
::
::    .hed: the first (trap vase)
::    .tal: the second (trap vase)
::
::    returns a merged (trap vase)
++  slat
  |=  [hed=(trap vase) tal=(trap vase)]
  ^-  (trap vase)
  =>  +<
  |.
  =+  [bed bal]=[$:hed $:tal]
  [[%cell p:bed p:bal] [q:bed q:bal]]
::  +shot: deferred slam
::
::    .gat: the gate to slam with the sample as a (trap vase)
::    .sam: the sample to slam with the gate
::
::    NOTE: this should never run inside of a trap. if it does, the builder
::    dependencies will leak into the result.
::
++  shot
  ~/  %shot
  |=  [gat=(trap vase) sam=(trap vase)]
  ^-  (trap vase)
  =/  [typ=type gen=hoon]
    :-  [%cell p:$:gat p:$:sam]
    [%cnsg [%$ ~] [%$ 2] [%$ 3] ~]
  =+  gun=(~(mint ut typ) %noun gen)
  =>  [typ=p.gun +<.$]
  |.
  [typ .*([q:$:gat q:$:sam] [%9 2 %10 [6 %0 3] %0 2])]
::
::  +swet: deferred +slap
::
::  NOTE: this is +swat but with a bug fixed that caused a space leak in
::  the resulting trap vases.
::
++  swet
  ~/  %swet
  |=  [tap=(trap vase) gen=hoon]
  ^-  (trap vase)
  =/  gun  (~(mint ut p:$:tap) %noun gen)
  =>  [gun=gun tap=tap]
  |.  ~+
  [p.gun .*(q:$:tap q.gun)]
::
++  is-hoon
  |=  pax=path
  ^-  ?
  =/  end  (rear pax)
  =/  tape  (trip end)
  =/  ext  (trip '.hoon')
  =/  tape-len  (lent tape)
  =/  ext-len  (lent ext)
  ?:  (lth tape-len ext-len)
    %.n
  =/  suffix  (slag (sub tape-len ext-len) tape)
  =(suffix ext)
::
::
++  is-dat
  |=  pax=path
  ^-  ?
  =('dat' (head pax))
--
