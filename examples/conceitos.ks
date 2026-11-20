// conceitos.ks: um passeio por tudo o que o Kestrel já tem.
// Compile e rode:  ksc examples/conceitos.ks && ksvm examples/conceitos.ksb

// ---------- Declarações de topo ----------

const LIMITE: int = 9;                // conhecido na compilação
const TITULO: text = "Kestrel";

enum Cor { Vermelho, Verde, Azul }                 // 0, 1, 2
enum Http { Ok = 200, NotFound = 404, Teapot }     // 200, 404, 405

struct Ponto {
    x: int;
    y: int;
}

struct Pessoa {
    nome: text;
    casa: Ponto;                      // struct dentro de struct
}

interface Forma {
    proc area(lent self) -> real;
    proc nome(lent self) -> text;
}

@impl(Forma)
class Circulo {
    raio: real;

    public proc novo(raio: real) -> Circulo {
        return Circulo { raio: raio };
    }

    @override
    public proc area(lent self) -> real {
        return 3.14 * self.raio ** 2;
    }

    @override
    public proc nome(lent self) -> text {
        return "círculo";
    }
}

@impl(Forma)
class Quadrado {
    lado: real;

    public proc novo(lado: real) -> Quadrado {
        return Quadrado { lado: lado };
    }

    @override
    public proc area(lent self) -> real {
        return self.lado * self.lado;
    }

    @override
    public proc nome(lent self) -> text {
        return "quadrado";
    }
}

class Conta {
    public nome: text;                // público
    saldo: real;                      // privado (padrão)
    hist[]: real;                     // array que pertence ao objeto

    public proc nova(nome: text) -> Conta {
        return Conta { nome: nome, saldo: 0.0, hist: {} };
    }

    public proc depositar(lent self, v: real) {
        self.saldo += v;
        append(self.hist, v);
    }

    public proc fechar(own self) -> real {
        s: real = self.saldo;
        burn self;
        return s;
    }
}

// ---------- Procs auxiliares ----------

// Aceita qualquer classe que implemente Forma (emprestada).
proc mostrar(lent f: Forma) {
    printf("  {f.nome()}: área {f.area()}\n");
}

// Cria um array e devolve a posse para quem chamou.
proc quadrados(n: int) -> int[] {
    v[]: int = claim n;
    for i: int = 0; i < n; i++ {
        v[i] = i * i;
    }
    return v;
}

// Recebe a posse (own): precisa consumir o array.
proc somarEQueimar(own v[]: int) -> int {
    total: int = 0;
    walk i in 0..len(v) - 1 {
        total += v[i];
    }
    burn v;
    return total;
}

// Soma um array simétrico emprestado (índices -k..+k).
proc somaSym(lent sym v[]: int, k: int) -> int {
    acc: int = 0;
    walk i in span k {
        acc += v[i];
    }
    return acc;
}

// Avisa a falha de propósito com NaT (o "null" do Kestrel).
proc dividir(a: int, b: int) -> int {
    if b == 0 {
        return NaT;
    }
    return a / b;
}

// tri: desvio em três caminhos.
proc sinal(x: int) -> text {
    r: text;
    tri x <=> 0 {
        - => r = "negativo";
        0 => r = "zero";
        + => r = "positivo";
    }
    return r;
}

// on NaT: o "try/catch" de uma linha. O bloco precisa divergir.
proc tentar(a: int, b: int) -> int {
    q: int = dividir(a, b) on NaT {
        printl("  divisão por zero tratada");
        return -1;
    };
    return q;
}

proc estourarTryte(t: tryte) -> int {
    u: tryte = t + 100 on NaT {
        printf("  {t} + 100 não cabe num tryte\n");
        return -1;
    };
    return u;
}

// ---------- Programa ----------

proc main() -> int {
    printl("=== " + TITULO + " ===");

    // 1. Números, literais e operadores
    printl("\n-- 1. Números --");
    a: int = 7;
    b: int = 3;
    printf("%d / %d = %d, resto %d\n", a, b, a / b, a % b);
    printf("2 ** 10 = {2 ** 10}, 0t+0- = {0t+0-}, 0vH = {0vH}\n");
    printf("-5 em ternário: {-5:t}, 100 em heptavintimal: {100:v}\n");
    printf("1 << 2 = {1 << 2}, 2 >> 1 = {2 >> 1}, 2 / 3 = {2 / 3}\n");
    a += 10;
    a++;
    a *= 2;
    printl("a depois de += ++ *=: " + toText(a));

    t: tryte = 300;
    grande: int = t;                  // ampliação implícita
    um: trit = (trit) 1;              // estreitamento exige cast
    r: real = (real) b / 3.0;
    printf("tryte {t} -> int {grande}, trit {um}, 3.0 / 3.0 = {r}\n");
    printf("1.0 / 3.0 = {1.0 / 3.0}, (int) 2.9 = {(int) 2.9}\n");

    final dobro: int = LIMITE * 2;    // final: não muda mais
    printl("dobro de LIMITE: " + toText(dobro));

    // 2. bool e trit (lógica de Kleene)
    printl("\n-- 2. bool e trit --");
    ok: bool = 5 > 2;
    talvez: trit = 0;
    printf("ok = {ok}, ok && talvez = {ok && talvez}, ok || talvez = {ok || talvez}\n");
    printf("!talvez = {!talvez}, 3 <=> 5 = {3 <=> 5}\n");
    vazio: int;                       // sem valor: NaT
    printf("vazio é NaT? {isNaT(vazio)}\n");

    // 3. Texto e caracteres
    printl("\n-- 3. Texto --");
    nome: text = "Kestrel";
    saudacao: text = "Olá, " + nome + "!";
    printl(saudacao);
    printf("len = {len(nome)}, sub = {sub(nome, 0, 3)}, primeira letra: {nome[0]:c}\n");
    n: int = toInt("123") + 1;
    printl("toInt(\"123\") + 1 = " + toText(n));
    c: tryte = 'K';
    printf("'K' vale {c}, maiúscula? {isUpper(c)}, dígito? {isDigit('7')}\n");
    emit('O'); emit('k'); emit('!');
    printl();

    // 4. Controle de fluxo
    printl("\n-- 4. Controle --");
    walk x in -2..2 {
        printl("  " + toText(x) + " é " + sinal(x));
    }

    if a > 100 {
        printl("a é grande");
    } else if a == 0 {
        printl("a é zero");
    } else {
        printl("a é pequeno: " + toText(a));
    }

    i: int = 0;
    while + {                         // laço infinito, sai com break
        i++;
        if i % 2 == 0 {
            continue;
        }
        if i > 7 {
            break;
        }
        print(i);
        print(" ");
    }
    printl("(ímpares com while)");

    for k: int = 10; k > 0; k -= 3 {
        print(k);
        print(" ");
    }
    printl("(for com passo -3)");

    letra: tryte = 'e';
    match letra {
        'a' | 'e' | 'i' | 'o' | 'u' => printl("'e' é vogal");
        else => printl("consoante");
    }

    // 5. Arrays e posse
    printl("\n-- 5. Arrays --");
    v[]: int = {5, 1, 4};
    append(v, 9);                     // [5, 1, 4, 9]
    insert(v, 0, 0);                  // [0, 5, 1, 4, 9]
    remove(v, 1);                     // [0, 1, 4, 9]
    ultimo: int = pop(v);             // [0, 1, 4]
    printf("v = {v}, pop deu {ultimo}, len = {len(v)}\n");
    resize(v, 5);                     // completa com zeros
    printl(v);
    replace(v, 0..0, 2..2);           // troca v[0] com v[2]
    printl(v);
    clear(v);
    printf("depois de clear: {v}\n");
    burn v;                           // libera o array

    nomes[]: text = {"Ana", "Bia"};
    append(nomes, "Caio");
    printl(nomes);
    burn nomes;

    sym s[]: int = {-8, -1, 0, 1, 8}; // índices -2..+2
    soma: int = somaSym(lend s, 2);   // empresta
    printf("s[-2] = {s[-2]}, s[0] = {s[0]}, soma = {soma}\n");
    burn s;

    q[]: int = quadrados(5);          // recebe a posse
    printf("quadrados = {q}\n");
    total: int = somarEQueimar(give q); // passa a posse adiante
    printl("soma dos quadrados: " + toText(total));

    // 6. enum e struct
    printl("\n-- 6. enum e struct --");
    cor: Cor = Cor.Verde;
    printf("cor = {cor}, número = {(int) cor}, Http.Teapot = {(int) Http.Teapot}\n");
    match cor {
        Cor.Vermelho => printl("pare");
        Cor.Verde => printl("siga");
        Cor.Azul => printl("?");
    }

    p1: Ponto = Ponto { x: 1, y: 2 };
    p2: Ponto = p1;                   // cópia
    p2.x = 10;
    printf("p1 = {p1}, p2 = {p2}\n");
    pessoa: Pessoa = Pessoa { nome: "Ana", casa: Ponto { x: 3, y: 4 } };
    pessoa.casa.y++;
    printl(pessoa);

    pts[]: Ponto = claim 2;
    pts[1].x = 5;
    printf("pts[1] = {pts[1]}\n");
    burn pts;

    // 7. Classes e interfaces
    printl("\n-- 7. Classes --");
    conta: Conta = Conta.nova("Ana");
    conta.depositar(10.0);
    conta.depositar(2.5);
    printl(conta);
    printl("dono: " + conta.nome);
    saldo: real = conta.fechar();     // consome o objeto
    printf("saldo final: {saldo}\n");

    circ: Circulo = Circulo.novo(2.0);
    quad: Quadrado = Quadrado.novo(3.0);
    mostrar(lend circ);
    mostrar(lend quad);
    burn circ;
    burn quad;

    // 8. NaT e falhas tratadas
    printl("\n-- 8. NaT --");
    r1: int = tentar(10, 2);
    r2: int = tentar(10, 0);
    r3: int = estourarTryte(300);
    printf("tentar(10, 2) = {r1}, tentar(10, 0) = {r2}, estourarTryte(300) = {r3}\n");

    return 0;
}
