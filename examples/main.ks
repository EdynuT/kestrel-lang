struct Aluno {
    nome: text;
    nota1: real;
    nota2: real;
}

proc media(aluno: Aluno) -> real {
    media: real = (aluno.nota1 + aluno.nota2) / 2.0;
    return media;
}

// Recebe a posse do array, devolve a memória e retorna 1.
proc take(own v[]: int) -> int {
    burn v;
    return 1;
}

proc main() {
    nome[]: text = {"Ana", "Bia"};
    printl(nome);

    turma[]: Aluno = claim 3;
    turma[0] = Aluno {nome: "Ana", nota1: 6.0, nota2: 4.5};
    turma[1] = Aluno {nome: "Bia", nota1: 8.5, nota2: 7.8};
    printf("%s\n", turma[0]);

    printf("A media de {turma[0].nome} é {media(turma[0])}\n");
    printf("A media de {turma[1].nome} é {media(turma[1])}\n");

    burn nome;
    burn turma;

    v[]: int = {1, 2, 3};
    x: trit = -1;
    while 1 {
        if x && take(give v) > 0 {
        }
    }
}
