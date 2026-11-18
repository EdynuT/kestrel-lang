# Kestrel — Especificação v0.1

- Código-fonte: `.ks` · Bytecode: `.ksb`
- Alvo: BET-VM, processador ternário balanceado de 27 trits
- Paradigma: imperativo estruturado, determinístico, sem GC, posse linear estrita
- Lógica: Kleene K₃ com propagação de veneno (NaT)

---

## 1. Dados

### 1.1 Trit e codificação BET

| Lógico | Símbolo | BET | Valor |
|---|---|---|---|
| Negativo / Falso | `-` | `10` | −1 |
| Neutro / Indeterminado | `0` | `00` | 0 |
| Positivo / Verdadeiro | `+` | `01` | +1 |
| Veneno (NaT) | `?` | `11` | ⊥ |

### 1.2 Tipos escalares (copiáveis)

| Tipo no código | Largura | Intervalo |
|---|---|---|
| `trit` | 1 trit | −1 .. +1 |
| `tryte` | 6 trits (t6) | −364 .. +364 |
| `word` | 27 trits (t27) | −3.812.798.742.493 .. +3.812.798.742.493 |
| `real` | 27 trits (t27) | ≈ ±10¹⁷⁴, com cerca de 10 dígitos decimais de precisão (§1.5) |
| `text` | referência | String literal imutável (§1.6) |

Neste documento, `t6` e `t27` indicam larguras. No código-fonte, os tipos se escrevem `tryte` e `word`.

### 1.3 NaT (veneno)

- O NaT vale para a **palavra inteira**. Basta um trit `?` para a palavra toda ser NaT.
- Qualquer operação com um operando NaT produz NaT.
- Na fonte, o NaT se escreve com a palavra-chave `NaT` ou com qualquer literal que contenha `?`.

### 1.4 Conversões

- A **ampliação** `trit → t6 → t27` é implícita e não perde informação.
- O **estreitamento** exige `as` (`x as tryte`, `x as trit`). Um valor fora do intervalo vira **NaT**.
- A conversão entre `word` e `real` é **sempre explícita**. `x as real` arredonda para o valor mais próximo, e `r as word` trunca em direção a zero (fora do intervalo vira NaT).

### 1.5 `real` (ponto flutuante ternário)

```
word de 27 trits:  [ 6 trits expoente | 21 trits mantissa ]
valor = (mantissa / 3²⁰) × 3^expoente
```

- A mantissa é **normalizada**: o trit mais significativo é sempre `-` ou `+`, exceto no zero. Não há trit de sinal, porque o sinal está na própria mantissa.
- O expoente é um `tryte`, de −364 a +364.
- O arredondamento é sempre para o valor mais próximo, que é o efeito natural de descartar trits.
- Overflow e divisão por zero produzem **NaT**. **Não existe infinito.** Um underflow vira 0.
- Frações de 3 são exatas: `1.0 / 3.0` não tem erro de arredondamento.

### 1.6 `text` (string)

- É um literal imutável guardado no pool de strings do `.ksb`, como o `const char*` do C. É copiável e **não precisa de `burn`**.
- `s[i]` devolve o `tryte` da posição `i`. **O índice começa em 0**, como em C (exceção à indexação simétrica dos buffers). Índice fora de `0 .. len(s)-1` dispara **Trap**.
- `len(s)` devolve o tamanho como `word`.
- `print(s)` imprime o texto.
- Cada caractere precisa existir na tabela §4. Um caractere fora dela, como `@` ou `ç`, é **erro de compilação**.
- Para montar texto em tempo de execução, use um `own [tryte]`, como um array de `char` em C.

---

## 2. Literais

| Forma | Exemplo | Regra |
|---|---|---|
| Decimal | `42`, `-8` | Fora de ±3.812.798.742.493 é **erro de compilação** |
| Real | `3.14`, `-0.5`, `2.0` | Exige um dígito antes e depois do ponto, e tem o tipo `real` |
| Ternário `0t` | `0t+0-` (= 8) | 1 a 27 trits, sendo o mais significativo à esquerda |
| Heptavintimal `0v` | `0vH`, `0vm` | 1 a 9 dígitos: `0` = 0, `A`–`M` = +1..+13, `a`–`m` = −1..−13, `?` = NaT |
| Trit | `-`, `0`, `+` | Na posição de valor (argumentos, ramos do `tri`) |
| Caractere | `'A'`, `'a'`, `' '`, `'\n'` | Vale o `tryte` da tabela §4 (`'A'` = +1, `'a'` = −1, `' '` = 0) |
| Veneno | `NaT` | — |
| String | `"Hello\n"` | Tem o tipo `text`. Escapes: `\n` `\t` |

**Comentários:** `//` até o fim da linha.

Cada dígito `0v` vale 3 trits, então uma palavra `t27` cabe em 9 dígitos. Um literal `0t` termina no primeiro caractere que não é trit, de modo que `0t+-` seguido de `- y` precisa de espaço entre eles.

---

## 3. Operadores

Em ordem de precedência, da menor para a maior:

| Prec. | Operador | Semântica |
|---|---|---|
| 1 | `\|\|` | Disjunção de Kleene (`max`) |
| 2 | `&&` | Conjunção de Kleene (`min`) |
| 3 | `<=>` | Comparação de 3 vias, resultado `trit` (−, 0, +) |
| 4 | `+` `-` | Soma e subtração |
| 5 | `*` `/` `%` | Multiplicação, divisão e resto |
| 6 | `<<` `>>` | Deslocamento ternário (×3ᵏ e ÷3ᵏ) |
| 7 | `~` `-` (unário), `as` | Negação e conversão |
| 8 | `**` | Potência, associativa à direita (`2 ** 3 ** 2 = 2 ** 9`) |

Regras:

- **Overflow** além da largura do tipo resultante produz **NaT**. Não existe wrap-around.
- **`/`** trunca em direção a zero. Divisão por zero produz NaT.
- **`%`** é o resto da divisão. O sinal segue o dividendo, e vale `a = (a/b)·b + a%b`. Resto por zero produz NaT.
- **`>>`** descarta os trits inferiores, o que equivale a arredondar para o inteiro mais próximo. Exemplo: `2 >> 1 = 1`, enquanto `2 / 3 = 0`.
- **`<<`** produz NaT se algum trit não nulo passar da largura do tipo. Com `k ≥ 27`, o resultado é `0` se o operando for 0 e NaT caso contrário. Com `k < 0` ou `k` NaT, o resultado é NaT.
- **`~x`** e **`-x`** são idênticos em ternário balanceado e compilam para o mesmo opcode `NEG`.
- **`**`** aceita `word ** word` e `real ** word`. O expoente é sempre `word`. Um expoente negativo em `word` produz NaT, e `0 ** 0 = 1`. Como em Python, `-2 ** 2 = -(2 ** 2) = -4`.
- **`real`** aceita `+ - * / ** <=> ~ -`. Os operadores `%`, `<<`, `>>`, `&&` e `||` sobre `real` são **erro de tipo**, assim como misturar `word` e `real` numa conta sem `as`.
- **`&&`** e **`||`** são **estritos**: os dois lados são sempre avaliados, sem curto-circuito. Por isso `- && NaT = NaT`. Os operandos são do tipo `trit`. Usar `tryte` ou `word` é **erro de tipo**; para comparar números, use `<=>`.

### 3.1 Tabela de verdade K₃

| A | B | `A && B` | `A \|\| B` | `~A` |
|---|---|---|---|---|
| − | − | − | − | + |
| − | 0 | − | 0 | + |
| − | + | − | + | + |
| 0 | 0 | 0 | 0 | 0 |
| 0 | + | 0 | + | 0 |
| + | + | + | + | − |

---

## 4. Caracteres (`t6` simétrico)

| Valor | Caractere |
|---|---|
| 0 | espaço (memória expurgada aparece em branco) |
| +1 .. +26 | `A`–`Z` |
| −1 .. −26 | `a`–`z` (por isso `~'A' == 'a'`) |
| +27 .. +36 | `0`–`9` |
| −27 .. −43 | `\n` `\t` `.` `,` `:` `;` `!` `?` `-` `+` `/` `_` `=` `(` `)` `[` `]` |
| demais | reservados. `emit` imprime `#` |

Atribuição exata da faixa de pontuação: −27 `\n`, −28 `\t`, −29 `.`, −30 `,`, −31 `:`, −32 `;`, −33 `!`, −34 `?`, −35 `-`, −36 `+`, −37 `/`, −38 `_`, −39 `=`, −40 `(`, −41 `)`, −42 `[`, −43 `]`.

---

## 5. Memória simétrica

- `claim N` exige `N` ímpar e positivo (`N = 2k+1`). O ponteiro aponta para o centro `[0]`, e os índices válidos vão de `-k` a `+k`.
- As células nascem zeradas. O tipo da célula vem do tipo do buffer (`tryte` ou `word`).
- `N` par, `N ≤ 0`, `N` NaT ou índice fora de `[-k, +k]` disparam **Trap**.

## 6. Buffers e posse linear

| Tipo | Natureza | Pode |
|---|---|---|
| `own [T]` | Linear, dono único, não copiável | Ler, escrever, `give`, `burn`, `yield` |
| `lent [T]` | Empréstimo, **segunda classe** | Ler e escrever |

`T` pode ser `tryte` ou `word`. Não existe aritmética de ponteiros.

| Palavra | Equivale a (C/Rust) | Efeito |
|---|---|---|
| `claim N` | `malloc` | Aloca `N` células, com índices de `-k` a `+k` |
| `burn x` | `memset 0` + `free` | Zera o bloco, e o slot do dono vira NaT |
| `give x` | `move` | Transfere a posse, e o slot do dono vira NaT |
| `lend x` | `&mut x` | Empresta durante uma chamada |

### 6.1 Regras do `lent`

- Existe **somente como parâmetro** de `proc`, e é criado com `lend x` no argumento.
- Não pode ser retornado nem guardado em variável local.
- Não aceita `burn` nem `give`.
- `f(lend b, give b)` na mesma chamada é erro estático.

### 6.2 Regras do `own`

Cada `own` deve ser consumido **exatamente uma vez** em todo caminho de execução, por `burn x`, `give x` ou `yield x`.

Erros de compilação:

- Usar a variável depois de `give` ou `burn`.
- Chegar a `}` ou `yield` com um `own` vivo (vazamento).
- Copiar sem `give` (`c: own [word] = b;`).
- Guardar `claim` em uma variável que não seja `own`.
- Atribuir a um `own` que ainda está vivo.
- Atribuir a um `own` consumido por `give`. O nome só pode receber um novo `claim` **depois de `burn`**.
- Passar a um parâmetro `own` um argumento sem `give`.
- Usar como comando (`f();`) uma expressão de tipo `own`, o que descartaria o ponteiro.

### 6.3 Fluxo e linearidade

- **`tri`**: todos os ramos que não divergem devem terminar com o **mesmo** estado linear. Um ramo que diverge (`yield`, `break`, `continue`) precisa ter consumido o que for exigido naquele ponto de saída.
- **`walk`**: o corpo não pode consumir um `own` declarado fora do laço. Antes de `break` ou `continue`, todo `own` declarado dentro do corpo deve estar consumido.
- **`on NaT`**: o bloco é obrigado a divergir (§8.6).

## 7. Classes de falha

| Classe | Causas | Efeito |
|---|---|---|
| **NaT** (dado) | Overflow, divisão por zero, `as` fora do intervalo, `input` inválido | O valor vira NaT e pode ser interceptado por `on NaT` |
| **Trap** (estrutura) | `claim` inválido, índice fora dos limites, `tri` sobre NaT, limite de `walk` NaT, ponteiro inválido, `burn` duplo | A VM **zera todo o heap e os registradores** e aborta com código ≠ 0 |

## 8. Sintaxe

A proporção é cerca de 80% C++/Rust para 20% pseudo-assembly. Os toques de assembly são **opcionais**, e cada um tem uma forma equivalente em C++/Rust.

### 8.1 `proc` e `yield`

```
proc sum(v: lent [word], k: word) -> word { ... yield acc; }
proc main() -> word { ... yield 0; }
```

O ponto de entrada é `main`. Um `proc` sem `-> T` usa `yield;` sem valor.

### 8.2 Declaração e atribuição

```
x: word = 5;          // tipo explícito
y := x * 3;           // tipo inferido do valor
x = x + 1;            // atribuição
```

Cada declaração ocupa um slot próprio, então um `x` declarado num bloco interno não sobrescreve o `x` de fora.

### 8.3 Registrador fixo `@R` (assembly)

```
acc: word @R0 = 0;    // acc mora no registrador R0
```

- Os registradores válidos são `@R-4` .. `@R+4`, e cada um aceita no máximo uma variável fixada por `proc`.
- A fixação vale só para escalares. Sem `@R`, o compilador escolhe o lugar.
- O `dump` mostra o nome da variável ao lado do registrador.

### 8.4 Instruções como comandos (assembly)

| Forma | Significa |
|---|---|
| `add d, s;` | `d = d + s` |
| `add d, a, b;` | `d = a + b` |
| `sub`, `mul`, `div`, `mod`, `pow`, `shl3`, `shr3` | Mesma forma de 2 ou 3 operandos |
| `neg d;` | `d = -d` |
| `mov d, s;` | `d = s` |

`d` é uma variável ou um `b[i]`. A semântica é a mesma das expressões (overflow vira NaT etc.). É só outra forma de escrever.

### 8.5 `tri` (desvio triplo)

```
tri s <=> 10 {
    - => print("less"),
    0 => print("equal"),
    + => { print("greater"); show(s); }
}
tri cmp3 s, 10 { ... }     // forma assembly, idêntica
```

- A expressão é do tipo `trit`. Os três ramos são obrigatórios e cada um aparece uma única vez.
- Um ramo é um comando seguido de `,` ou um bloco `{ }`.
- Se a expressão for NaT, ocorre **Trap**.

### 8.6 `walk` (laço simétrico)

```
walk i in -2..5 { ... }
walk i in span k { ... }   // o mesmo que -k..+k
```

- O intervalo é inclusivo e o passo é +1. Se `a > b`, o corpo não executa nenhuma vez.
- `a` e `b` são avaliados **uma única vez**, antes da primeira iteração. Se algum for NaT, ocorre **Trap**.
- `break` e `continue` são permitidos.
- `i` é `word` e só pode ser lido dentro do laço.

### 8.7 `on NaT` (barreira de veneno)

```
s: word = sum(lend b, 2) on NaT { burn b; yield -1; };
```

- Se a expressão não for NaT, `s` recebe o valor.
- Caso contrário, executa o bloco, que precisa divergir (`yield`, `break` ou `continue`).
- Depois do `;`, `s` com certeza não é NaT.

### 8.8 E/S

| Comando | Efeito |
|---|---|
| `print(s)` | Imprime um `text` |
| `len(s)` | Tamanho de um `text` |
| `show(x)` | Imprime um valor em decimal, `0t` e `0v` |
| `emit(c)` | Imprime um `tryte` como caractere (tabela §4) |
| `input()` | Lê um `word` (decimal, `0t` ou `0v`). Entrada inválida vira NaT |
| `dump("label")` | Mostra os registradores e o heap |

## 9. BET-VM

- **Representação interna:** `i64`, com o NaT como sentinela `i64::MIN`. A forma `Word27` (BET em `u64`, 54 bits) só aparece no `.ksb`, no `dump` e na formatação `0t`/`0v`.
- **Pilha de avaliação** compartilhada, mais um **frame** por chamada.
- **Locais:** os índices 0..8 aparecem como os registradores simétricos `R-4 .. R+4`. Do índice 9 em diante, ficam na área de spill do frame, sem limite. Cada `let` recebe um slot próprio, então dois `x` em escopos diferentes não se sobrepõem.
- **Ponteiro:** `(índice: u32, geração: u32)`. O `burn` zera o bloco, incrementa a geração e devolve o bloco à free-list. Um acesso com geração antiga dispara **Trap**.

### 9.1 ISA

| Categoria | Opcodes |
|---|---|
| Pilha e locais | `PUSHC k` (word), `PUSHR k` (real), `LOAD s`, `STORE s`, `MOVE s`, `POP` |
| ALU (inteira) | `ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `POW`, `NEG`, `SHL3`, `SHR3`, `CMP3`, `KAND`, `KOR` |
| ALU (real) | `FADD`, `FSUB`, `FMUL`, `FDIV`, `FPOW`, `FNEG`, `FCMP3` |
| Texto | `PUSHS k`, `TLEN`, `TIDX` |
| Largura | `NARROW6`, `NARROW1`, `ITOF`, `FTOI` (overflow vira NaT) |
| Memória | `ALLOC`, `LDIDX`, `STIDX`, `PURGE s` |
| Fluxo | `JMP a`, `JNAT a`, `BR3 a- a0 a+`, `CALL f n`, `RET` |
| E/S | `INPUT`, `EMIT`, `PRINT`, `SHOW`, `DUMP str` |

**Codificação:** cada instrução tem 1 byte de opcode seguido de operandos de tamanho fixo, em little-endian. Os tipos são `k`, `f` e `a` (`u32`), `s` (`u16`) e `n` (`u8`). As faixas são: pilha `0x0_`, ALU inteira `0x1_`, ALU real `0x2_`, texto e conversão `0x3_`, memória `0x4_`, fluxo `0x5_` e E/S `0x6_`. A tabela completa está em `crates/kestrel-core/src/isa.rs`.

## 10. Formato `.ksb`

Todos os campos são little-endian.

```
Header (24 B)
  magic        [u8;4] = "K3B\x01"
  version      u16
  flags        u16
  entry_fn     u32          índice de main
  const_count  u32
  str_count    u32
  fn_count     u32
Const pool     const_count × u64     (Word27 BET)
String pool    str_count × (u32 len + bytes UTF-8)
Functions      fn_count ×
  name_len u16, name UTF-8
  param_count u8, local_count u16
  code_len u32 (em bytes)
  code: opcode u8 + operandos fixos
  [se flags & 1] local_count × (u16 len + nome UTF-8)
```

- **Bit 0 de `flags`** indica que o arquivo traz os nomes das variáveis, para o `dump` mostrar `R0 (acc)`.
- **Verificação na leitura:** o arquivo é rejeitado se algum salto cair fora do início de uma instrução, se um índice de constante, string, função ou slot não existir, ou se o número de argumentos de uma chamada não bater com os parâmetros da função chamada.

Os saltos usam um **offset absoluto em bytes** (`u32`) dentro do código da própria função.

---

## 11. Gramática (EBNF)

```ebnf
program    = { const_decl | proc_decl } ;
const_decl = "const" IDENT ":" scalar "=" expr ";" ;
proc_decl  = "proc" IDENT "(" [ param { "," param } ] ")" [ "->" type ] block ;
param      = IDENT ":" type ;
type       = scalar | "text" | ( "own" | "lent" ) "[" elem "]" ;
scalar     = "trit" | "tryte" | "word" | "real" ;
elem       = "tryte" | "word" ;
REG        = "R-4" | "R-3" | "R-2" | "R-1" | "R0" | "R+1" | "R+2" | "R+3" | "R+4" ;

block      = "{" { stmt } "}" ;
stmt       = IDENT ":" type [ "@" REG ] "=" expr [ on_nat ] ";"
           | IDENT ":=" expr [ on_nat ] ";"
           | place "=" expr ";"
           | instr ";"
           | "burn" IDENT ";"
           | "yield" [ expr ] ";"
           | "break" ";" | "continue" ";"
           | "tri" tri_head "{" arm arm arm "}"
           | "walk" IDENT "in" range block
           | io ";"
           | expr ";" ;
on_nat     = "on" "NaT" block ;
place      = IDENT [ "[" expr "]" ] ;
instr      = ( "add" | "sub" | "mul" | "div" | "mod" | "pow" | "shl3" | "shr3" ) place "," expr [ "," expr ]
           | "neg" place
           | "mov" place "," expr ;
tri_head   = expr | "cmp3" expr "," expr ;
arm        = ( "-" | "0" | "+" ) "=>" ( block | stmt_noterm "," ) ;
range      = expr ".." expr | "span" expr ;
io         = "print" "(" expr ")" | "show" "(" expr ")" | "emit" "(" expr ")"
           | "dump" "(" STRING ")" ;

expr       = or_expr ;
or_expr    = and_expr { "||" and_expr } ;
and_expr   = cmp_expr { "&&" cmp_expr } ;
cmp_expr   = add_expr [ "<=>" add_expr ] ;
add_expr   = mul_expr { ( "+" | "-" ) mul_expr } ;
mul_expr   = sh_expr  { ( "*" | "/" | "%" ) sh_expr } ;
sh_expr    = unary { ( "<<" | ">>" ) unary } ;
unary      = ( "~" | "-" ) unary | power [ "as" scalar ] ;
power      = postfix [ "**" unary ] ;
postfix    = primary [ "[" expr "]" ] ;
primary    = literal | IDENT | IDENT "(" [ arg { "," arg } ] ")"
           | "claim" expr | "input" "(" ")" | "len" "(" expr ")" | "(" expr ")" ;
arg        = [ "give" | "lend" ] expr ;
```

`stmt_noterm` é um `stmt` sem o `;` final.

### Exemplo

```
proc sum(v: lent [word], k: word) -> word {
    acc: word @R0 = 0;
    walk i in span k {
        add acc, v[i];             // acc = acc + v[i]
    }
    yield acc;
}

proc main() -> word {
    b: own [word] = claim 5;       // índices -2..+2
    walk i in span 2 {
        b[i] = i * i;
    }

    s: word @R+1 = sum(lend b, 2) on NaT { burn b; yield -1; };
    shl3 s, 1;                     // s = s * 3

    tri cmp3 s, 10 {
        - => print("less"),
        0 => print("equal"),
        + => print("greater"),
    }
    burn b;
    yield 0;
}
```

## 12. Pendências

Nenhuma. Especificação v0.1 fechada.
